#[test]
fn c04_approved_identity_recovery_retires_exact_incumbent_without_grant_change() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c04");
    let operation_id = "ctxop-v23-c04-seed";
    let fence = seed_stale_identity(&fixture, &pane, operation_id, false);
    let recovery_id = "ctxop-v23-c04-recovery";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let grant_before = master_status(&fixture, &pane);
    let before_events = fs::read(fixture.host_journal()).unwrap();

    let recovered = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let payload = success_payload(&recovered);
    assert_eq!(
        payload["result"]["invocation"], "approved_recovery",
        "{payload}"
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
    let new_binding = &payload["result"]["snapshot"]["binding"];
    let new_binding_id = new_binding["binding_id"]
        .as_str()
        .expect("approved recovery returns real binding id");
    let new_endpoint_generation = new_binding["endpoint_generation"]
        .as_u64()
        .expect("approved recovery returns an endpoint generation");
    assert_eq!(
        new_binding_id, fence.binding_id,
        "approved recovery must retain the stable binding id: {payload}"
    );
    assert!(
        new_endpoint_generation > fence.endpoint_generation,
        "{payload}"
    );
    assert_eq!(
        master_status(&fixture, &pane),
        grant_before,
        "identity recovery without grant approval must not create or replace master authority: {payload}"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &fence.binding_id,
            fence.endpoint_generation
        ),
        1,
        "the exact old binding version must be retired once"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            new_binding_id,
            new_endpoint_generation
        ),
        1,
        "exactly one newer binding version must commit"
    );
    assert!(
        fs::read(&fixture.host_journal()).unwrap().len() > before_events.len(),
        "recovery writes the new durable binding"
    );
    assert_eq!(
        fs::read(&fence.identity_path).expect("daemon overwrites the fixture cache")
            != fence.stale_identity_bytes,
        true,
        "the stale local credential must be replaced by the daemon receipt"
    );
}

#[test]
fn c05_minimal_approval_reads_current_incumbent_without_second_approval() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c05");
    let operation_id = "ctxop-v23-c05-seed";
    let fence = seed_stale_identity(&fixture, &pane, operation_id, false);
    let recovery_id = "ctxop-v23-c05-recovery";

    let replacement_id = "ctxop-v23-c05-replacement";
    let minimal_approval = json!({
        "decision": "approved",
        "target_identity": fence.worker_id,
        "project_scope": canonical_root(&fixture),
        "app_scope_id": "appserver-cli",
        "action": "restore_identity",
        "authorization": "user explicitly restored this pane identity"
    });
    let replacement = run_context_cli(
        &fixture,
        Some(&pane),
        Some(replacement_id),
        None,
        None,
        Some(&minimal_approval),
        None,
        None,
        false,
    );
    let replacement = success_payload(&replacement);
    assert_eq!(
        replacement["result"]["outcome"], "completed",
        "{replacement}"
    );
    let host_before = fs::read(fixture.host_journal()).expect("host journal after replacement");
    let identity_before = fs::read(&fence.identity_path).expect("identity after replacement");
    let changed = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        Some(&minimal_approval),
        None,
        None,
        false,
    );
    let changed_payload = success_payload(&changed);
    assert_eq!(
        changed_payload["result"]["outcome"], "completed",
        "one explicit intent must read the current incumbent and continue: {changed_payload}"
    );
    assert!(
        !text(&changed).contains("APPROVAL_STALE_CONFLICT"),
        "current incumbent read must not require a second approval: {}",
        text(&changed)
    );
    assert!(
        changed_payload["result"]["snapshot"]["binding"]["endpoint_generation"]
            .as_u64()
            .is_some_and(|generation| {
                generation
                    > replacement["result"]["snapshot"]["binding"]["endpoint_generation"]
                        .as_u64()
                        .expect("replacement binding has a generation")
            }),
        "{changed_payload}"
    );
    assert_ne!(
        fs::read(&fence.identity_path).expect("stale cache replaced"),
        identity_before,
        "the second explicit recovery must write its committed identity receipt"
    );
    assert_eq!(
        fs::read(fixture.host_journal()).unwrap().len() > host_before.len(),
        true,
        "a new explicit recovery operation must append its durable projection"
    );
    // Let the resident timer classify the newly recovered generation before
    // measuring replay purity. Its scan receipt is a separate journal event.
    let current_binding = &changed_payload["result"]["snapshot"]["binding"];
    assert!(
        wait_until(Duration::from_secs(10), || {
            let rows = read_lines(&fixture.project_journal());
            rows.iter()
                .rev()
                .find(|row| {
                    row["ev"] == "GlobalRuntimeBindingLedgerClassified"
                        && row["record"]["binding_id"] == current_binding["binding_id"]
                })
                .is_some_and(|row| {
                    row["record"]["endpoint_generation"] == current_binding["endpoint_generation"]
                        && row["record"]["state"] == "Live"
                        && row["record"]["probe_state"] == "Live"
                        && rows.iter().any(|receipt| {
                            receipt["ev"] == "GlobalLedgerScanReceiptRecorded"
                                && receipt["receipt"]["scanned_ms"] == row["record"]["classified_ms"]
                        })
                })
        }),
        "the recovered binding must have its real resident classification and scan receipt"
    );
    let before_replay = (
        fs::read(fixture.host_journal()).unwrap(),
        fs::read(fixture.project_journal()).unwrap(),
    );
    let identity_before_replay = fs::read(&fence.identity_path).unwrap();
    let replay = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        Some(&minimal_approval),
        None,
        None,
        false,
    );
    let replay = success_payload(&replay);
    assert_eq!(replay["result"]["outcome"], "completed");
    for field in ["operation_id", "outcome", "phase", "committed_phases"] {
        assert_eq!(
            replay["result"][field], changed_payload["result"][field],
            "replay {field}: {replay}"
        );
    }
    assert!(
        replay["result"]["snapshot"].is_null(),
        "replay must not reconcile: {replay}"
    );
    assert_eq!(
        (
            fs::read(fixture.host_journal()).unwrap(),
            fs::read(fixture.project_journal()).unwrap()
        ),
        before_replay
    );
    assert_eq!(
        fs::read(&fence.identity_path).unwrap(),
        identity_before_replay
    );
    let route = fixture.run_ok(
        &["route", "resolve", "--pane-id", &pane.pane_id],
        Some(&pane),
    );
    assert_eq!(route["endpoint_generation"], 3, "{route}");
    assert_eq!(
        route["binding_id"],
        changed_payload["result"]["snapshot"]["binding"]["binding_id"]
    );
}

#[test]
fn c06_grant_separation_requires_independent_grant_approval_and_preserves_grant_without_it() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c06");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c06-seed", true);
    let grant_before = master_status(&fixture, &pane);
    let grant_id = grant_before["grant_id"]
        .as_str()
        .expect("seed master status has grant id")
        .to_owned();
    let grant_generation = grant_before["grant_generation"]
        .as_u64()
        .expect("seed master status has grant generation");
    let recovery_id = "ctxop-v23-c06-identity-only";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let identity_only = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let payload = success_payload(&identity_only);
    assert_eq!(
        payload["result"]["invocation"], "approved_recovery",
        "{payload}"
    );
    let recovered_binding = &payload["result"]["snapshot"]["binding"];
    let recovered_binding_id = recovered_binding["binding_id"]
        .as_str()
        .expect("identity-only recovery returns a binding id");
    let recovered_endpoint_generation = recovered_binding["endpoint_generation"]
        .as_u64()
        .expect("identity-only recovery returns an endpoint generation");
    assert_eq!(
        recovered_binding_id, fence.binding_id,
        "identity-only recovery must retain the stable binding id: {payload}"
    );
    assert!(
        recovered_endpoint_generation > fence.endpoint_generation,
        "identity-only recovery must commit a greater endpoint generation: {payload}"
    );
    let grant_after_identity = master_status(&fixture, &pane);
    assert_eq!(
        grant_after_identity["grant_id"], grant_id,
        "identity approval alone must not replace the grant resource: {grant_after_identity}"
    );
    assert_eq!(
        grant_after_identity["grant_generation"],
        json!(grant_generation),
        "{grant_after_identity}"
    );
    assert_eq!(
        grant_after_identity["binding_id"], recovered_binding["binding_id"],
        "identity approval alone must advance only the endpoint fence: {grant_after_identity}"
    );
    assert_eq!(
        grant_after_identity["endpoint_generation"], recovered_binding["endpoint_generation"],
        "identity approval alone must advance only the endpoint fence: {grant_after_identity}"
    );
    let mut expected_identity_grant = grant_before.clone();
    expected_identity_grant["binding_id"] = recovered_binding["binding_id"].clone();
    expected_identity_grant["endpoint_generation"] =
        recovered_binding["endpoint_generation"].clone();
    assert_eq!(
        grant_after_identity, expected_identity_grant,
        "identity-only recovery must preserve principal, scope, authority, and approval metadata"
    );

    // The second invocation carries both approvals. The identity approval alone
    // never authorized the grant owner; only this distinct grant approval does.
    let current_fence = identity_fence_from_status(&fixture, &pane);
    let combined_identity = identity_approval(&fixture, &pane, &current_fence, "replace_binding");
    let current_grant = grant_fence(&grant_after_identity);
    let combined_grant = grant_approval(&fixture, &pane, &current_fence, &current_grant);
    let combined_id = "ctxop-v23-c06-combined";
    let combined = run_context_cli(
        &fixture,
        Some(&pane),
        Some(combined_id),
        None,
        None,
        Some(&combined_identity),
        Some(&combined_grant),
        None,
        false,
    );
    let combined_payload = success_payload(&combined);
    assert_eq!(
        combined_payload["result"]["invocation"], "approved_recovery",
        "{combined_payload}"
    );
    let combined_binding = &combined_payload["result"]["snapshot"]["binding"];
    let combined_binding_id = combined_binding["binding_id"]
        .as_str()
        .expect("combined recovery returns a binding id");
    let combined_endpoint_generation = combined_binding["endpoint_generation"]
        .as_u64()
        .expect("combined recovery returns an endpoint generation");
    assert_eq!(
        combined_binding_id, current_fence.binding_id,
        "combined recovery must retain the stable binding id: {combined_payload}"
    );
    assert!(
        combined_endpoint_generation > current_fence.endpoint_generation,
        "combined recovery must commit a greater endpoint generation: {combined_payload}"
    );
    let grant_after_replacement = master_status(&fixture, &pane);
    assert_eq!(
        grant_after_replacement["grant_id"], grant_id,
        "the separately approved replacement must retain the master grant resource: {grant_after_replacement}"
    );
    assert_eq!(
        grant_after_replacement["grant_generation"],
        json!(grant_generation + 1),
        "the independent grant approval must advance the grant generation exactly once: {grant_after_replacement}"
    );
    assert_eq!(
        grant_after_replacement["binding_id"], combined_binding["binding_id"],
        "the replacement must commit the recovered endpoint fence: {grant_after_replacement}"
    );
    assert_eq!(
        grant_after_replacement["endpoint_generation"], combined_binding["endpoint_generation"],
        "the replacement must commit the recovered endpoint fence: {grant_after_replacement}"
    );

    // Grant approval without the identity owner fence is still refused, and it
    // creates no outer operation record.
    let grant_only_fence = identity_fence_from_status(&fixture, &pane);
    let grant_only_grant = grant_fence(&grant_after_replacement);
    let grant_only_approval = grant_approval(&fixture, &pane, &grant_only_fence, &grant_only_grant);
    let grant_only_id = "ctxop-v23-c06-grant-only";
    let grant_only_output = run_context_cli(
        &fixture,
        Some(&pane),
        Some(grant_only_id),
        None,
        None,
        None,
        Some(&grant_only_approval),
        None,
        false,
    );
    let grant_only_text = text(&grant_only_output);
    assert!(
        !grant_only_output.status.success(),
        "grant approval alone must not replace the identity owner: {grant_only_text}"
    );
    assert!(
        grant_only_text.contains("IDENTITY_APPROVAL_REQUIRED"),
        "grant separation must name the missing identity approval: {grant_only_text}"
    );
    assert!(operation_records(&fixture.host_journal(), grant_only_id).is_empty());
    assert_eq!(
        master_status(&fixture, &pane),
        grant_after_replacement,
        "grant-only refusal must not mutate authority"
    );
}

#[test]
fn c07_same_key_approved_recovery_replays_projection_without_duplicate_effects() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c07");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c07-seed", false);
    let operation_id = "ctxop-v23-c07-replay";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");

    let first = run_context_cli(
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
    let first_payload = success_payload(&first);
    let first_journal = fs::read(fixture.host_journal()).expect("host journal");
    let first_events = journal_operations(&fixture.host_journal(), operation_id);
    let first_binding = first_payload["result"]["snapshot"]["binding"]["binding_id"]
        .as_str()
        .expect("recovered binding id")
        .to_owned();
    let first_binding_generation = first_payload["result"]["snapshot"]["binding"]
        ["endpoint_generation"]
        .as_u64()
        .expect("recovered binding generation");

    let replay = run_context_cli(
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
    let replay_payload = success_payload(&replay);
    assert_eq!(
        replay_payload["result"]["outcome"], "completed",
        "{replay_payload}"
    );
    assert_eq!(
        replay_payload["result"]["operation_id"],
        json!(operation_id),
        "same-key replay must return the same committed projection: {replay_payload}"
    );
    assert_eq!(
        replay_payload["result"]["phase"], first_payload["result"]["phase"],
        "same-key replay must retain the original terminal phase: {replay_payload}"
    );
    assert_eq!(
        replay_payload["result"]["committed_phases"], first_payload["result"]["committed_phases"],
        "same-key replay must retain the original business receipts: {replay_payload}"
    );
    assert_eq!(
        fs::read(fixture.host_journal()).expect("host journal after replay"),
        first_journal,
        "same-key replay must not append another durable operation transition"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &first_binding,
            first_binding_generation
        ),
        1,
        "same-key replay must not bind the recovered identity twice"
    );
    assert_eq!(
        operation_records(&fixture.host_journal(), operation_id),
        first_events,
        "same-key replay must not add duplicate operation records"
    );
}

#[test]
fn c08_same_key_changed_intent_conflicts_before_side_effects_and_preserves_original() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c08");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c08-seed", false);
    let operation_id = "ctxop-v23-c08-key";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");

    let original = run_context_cli(
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
    let original_payload = success_payload(&original);
    let original_binding = original_payload["result"]["snapshot"]["binding"]["binding_id"]
        .as_str()
        .expect("recovered binding id")
        .to_owned();
    let original_binding_generation = original_payload["result"]["snapshot"]["binding"]
        ["endpoint_generation"]
        .as_u64()
        .expect("recovered binding generation");
    let original_records = operation_records(&fixture.host_journal(), operation_id);
    let original_journal = fs::read(fixture.host_journal()).expect("host journal before conflict");

    let changed_pane = second_pane(&fixture, "c08", pane.server_pid);
    let changed = run_context_cli(
        &fixture,
        Some(&changed_pane),
        Some(operation_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let changed_text = text(&changed);
    assert!(
        changed_text.contains("IDENTITY_OPERATION_INTENT_CONFLICT"),
        "changed intent must be typed before side effects: {changed_text}"
    );
    let changed_payload = typed_failure(&changed);
    assert_eq!(
        changed_payload["result"]["outcome"], "denied",
        "{changed_payload}"
    );
    assert_eq!(
        fs::read(fixture.host_journal()).expect("host journal after conflict"),
        original_journal,
        "intent conflict must not append durable state"
    );
    assert_eq!(
        operation_records(&fixture.host_journal(), operation_id),
        original_records,
        "the original admitted operation must remain unchanged"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &original_binding,
            original_binding_generation
        ),
        1,
        "the original binding must not be rebound after a conflicting key"
    );

    let query = fixture.context_query(operation_id, Some(&pane));
    assert_eq!(
        query["result"]["queried_operation"]["outcome"], "completed",
        "{query}"
    );
    assert_eq!(
        query["result"]["queried_operation"]["operation_id"],
        operation_id
    );
}

#[test]
fn c09_query_after_retired_credential_uses_only_retained_local_capability() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c09");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c09-seed", false);
    let operation_id = "ctxop-v23-c09-recovered";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let recovered = run_context_cli(
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
    let recovered_payload = success_payload(&recovered);
    let durable_outcome = recovered_payload["result"]["outcome"].clone();
    let durable_binding = recovered_payload["result"]["snapshot"]["binding"]["binding_id"]
        .as_str()
        .expect("recovered binding id")
        .to_owned();
    let durable_binding_generation = recovered_payload["result"]["snapshot"]["binding"]
        ["endpoint_generation"]
        .as_u64()
        .expect("recovered binding generation");
    let proof = fixture.proof(operation_id);
    let capability = proof["query_capability"]
        .as_str()
        .expect("proof retains query capability")
        .to_owned();
    let before = stable_surfaces(&fixture, &pane, operation_id);

    let cli_query = fixture.command(&["context", "--op", operation_id, "--query"], Some(&pane));
    let cli_payload = success_payload(&cli_query);
    assert_eq!(
        cli_payload["result"]["invocation"], "query",
        "{cli_payload}"
    );
    assert_eq!(cli_payload["result"]["action"], "query", "{cli_payload}");
    assert_eq!(
        cli_payload["result"]["queried_operation"]["outcome"], durable_outcome,
        "{cli_payload}"
    );
    assert_eq!(
        cli_payload["result"]["queried_operation"]["operation_id"],
        operation_id
    );
    let cli_text = text(&cli_query);
    assert_no_secret_text("CLI query stdout", &cli_text, &capability);
    assert_no_public_secret_surface("CLI query result", &cli_payload);
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &durable_binding,
            durable_binding_generation
        ),
        1,
        "query must not repeat the committed binding effect"
    );

    let mut mcp = Mcp::start(&fixture, Some(&pane));
    let mcp_result = mcp.call_context(9, json!({"operation_id": operation_id, "query": true}));
    assert_eq!(mcp_result["isError"], false, "{mcp_result}");
    let mcp_text = mcp_result["content"][0]["text"]
        .as_str()
        .expect("MCP query returns typed payload text");
    let mcp_payload: Value = serde_json::from_str(mcp_text).expect("MCP query payload is JSON");
    assert_eq!(
        mcp_payload["result"]["queried_operation"]["outcome"], durable_outcome,
        "{mcp_payload}"
    );
    assert_eq!(
        mcp_payload["result"]["queried_operation"]["operation_id"],
        operation_id
    );
    assert_no_secret_text("MCP query text", mcp_text, &capability);
    assert_no_public_secret_surface("MCP query result", &mcp_payload);
    mcp.finish();

    fixture.set_proof_capability(operation_id, "wrong-capability-c09");
    let denied = fixture.command(&["context", "--op", operation_id, "--query"], Some(&pane));
    let denied_text = text(&denied);
    assert!(
        denied_text.contains("IDENTITY_OPERATION_QUERY_DENIED"),
        "wrong capability must be typed without disclosure: {denied_text}"
    );
    assert!(!denied_text.contains("wrong-capability-c09"));
    fixture.set_proof_capability(operation_id, &capability);
    assert_eq!(stable_surfaces(&fixture, &pane, operation_id), before);
}

#[test]
fn c10_master_identity_restore_preserves_existing_grant() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c10");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c10-seed", true);
    let grant_before = master_status(&fixture, &pane);
    let grant_id = grant_before["grant_id"].clone();
    let grant_generation = grant_before["grant_generation"].clone();
    let recovery_id = "ctxop-v23-c10-restore";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let recovered = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let payload = success_payload(&recovered);
    assert_eq!(
        payload["result"]["invocation"], "approved_recovery",
        "{payload}"
    );
    let recovered_binding = &payload["result"]["snapshot"]["binding"];
    let recovered_binding_id = recovered_binding["binding_id"]
        .as_str()
        .expect("master identity restore returns a binding id");
    let recovered_endpoint_generation = recovered_binding["endpoint_generation"]
        .as_u64()
        .expect("master identity restore returns an endpoint generation");
    assert_eq!(
        recovered_binding_id, fence.binding_id,
        "master identity restore must retain the stable binding id: {payload}"
    );
    assert!(
        recovered_endpoint_generation > fence.endpoint_generation,
        "master identity restore must commit a greater endpoint generation: {payload}"
    );
    let grant_after = master_status(&fixture, &pane);
    assert_eq!(
        grant_after["grant_id"], grant_id,
        "master identity restore must retain the existing grant resource: {grant_after}"
    );
    assert_eq!(
        grant_after["grant_generation"], grant_generation,
        "master identity restore must retain the existing grant version: {grant_after}"
    );
    assert_eq!(
        grant_after["binding_id"], recovered_binding["binding_id"],
        "master identity restore must publish the recovered endpoint fence: {grant_after}"
    );
    assert_eq!(
        grant_after["endpoint_generation"], recovered_binding["endpoint_generation"],
        "master identity restore must publish the recovered endpoint fence: {grant_after}"
    );
    let mut expected_grant = grant_before.clone();
    expected_grant["binding_id"] = recovered_binding["binding_id"].clone();
    expected_grant["endpoint_generation"] = recovered_binding["endpoint_generation"].clone();
    assert_eq!(
        grant_after, expected_grant,
        "master identity restore must preserve principal, scope, authority, and approval metadata: {grant_after}"
    );

    let query = fixture.context_query(recovery_id, Some(&pane));
    assert_eq!(
        query["result"]["queried_operation"]["outcome"], "completed",
        "{query}"
    );
    assert_eq!(
        query["result"]["queried_operation"]["operation_id"], recovery_id,
        "the retained query must resolve the exact recovered operation: {query}"
    );
    assert_eq!(
        master_status(&fixture, &pane),
        expected_grant,
        "the retained authority readback must show the recovered endpoint with unchanged authority: {query}"
    );
}

#[test]
fn c11_dual_approval_commits_identity_and_grant_once_without_duplicate_effects() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c11");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c11-seed", true);
    let grant_before = master_status(&fixture, &pane);
    let grant_before_id = grant_before["grant_id"].clone();
    let grant_before_generation = grant_before["grant_generation"].clone();
    let operation_id = "ctxop-v23-c11-dual";
    let identity = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let grant = grant_approval(&fixture, &pane, &fence, &grant_fence(&grant_before));
    let route_before = thread_route_event_count(&fixture.project_journal());

    let output = run_context_cli(
        &fixture,
        Some(&pane),
        Some(operation_id),
        None,
        None,
        Some(&identity),
        Some(&grant),
        None,
        false,
    );
    let payload = success_payload(&output);
    assert_eq!(
        payload["result"]["invocation"], "approved_recovery",
        "{payload}"
    );
    assert_eq!(
        payload["result"]["committed_phases"],
        json!([
            "approval_decision",
            "nested_register",
            "route",
            "credential",
            "grant",
            "lease",
            "context_complete"
        ]),
        "{payload}"
    );
    let new_binding = &payload["result"]["snapshot"]["binding"];
    let new_binding_id = new_binding["binding_id"]
        .as_str()
        .expect("dual approval returns a binding id")
        .to_owned();
    let new_endpoint_generation = new_binding["endpoint_generation"]
        .as_u64()
        .expect("dual approval returns an endpoint generation");
    assert_eq!(
        new_binding_id, fence.binding_id,
        "dual approval must retain the stable binding id: {payload}"
    );
    assert!(
        new_endpoint_generation > fence.endpoint_generation,
        "dual approval must commit a greater endpoint generation: {payload}"
    );

    let grant_after = master_status(&fixture, &pane);
    assert_eq!(
        grant_after["grant_id"], grant_before_id,
        "the grant owner must retain the same grant resource: {grant_after}"
    );
    assert_eq!(
        grant_after["grant_generation"],
        json!(grant_before_generation.as_u64().unwrap() + 1),
        "the grant owner must advance the grant version exactly once: {grant_after}"
    );
    assert_eq!(
        grant_after["binding_id"], new_binding_id,
        "the grant owner must publish the recovered endpoint fence: {grant_after}"
    );
    assert_eq!(
        grant_after["endpoint_generation"],
        json!(new_endpoint_generation),
        "the grant owner must publish the recovered endpoint fence: {grant_after}"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &fence.binding_id,
            fence.endpoint_generation
        ),
        1,
        "the exact approved incumbent binding version must be retired once"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &new_binding_id,
            new_endpoint_generation
        ),
        1,
        "exactly one newer binding version must commit"
    );
    assert_eq!(
        thread_route_event_count(&fixture.project_journal()),
        route_before + 1,
        "dual approval must publish the recovered route once"
    );
    let records = operation_records(&fixture.host_journal(), operation_id);
    let completed_records: Vec<_> = records
        .iter()
        .filter(|record| record["phase"] == "completed")
        .collect();
    assert_eq!(
        completed_records.len(),
        1,
        "the outer lifecycle must complete once: {records:?}"
    );
    let business_receipts = completed_records[0]["business_receipts"]
        .as_array()
        .expect("completed outer record has business receipts");
    assert_eq!(
        business_receipts.last().and_then(Value::as_str),
        Some("context_complete"),
        "the final business receipt must be context_complete: {records:?}"
    );
    assert_eq!(
        business_receipts
            .iter()
            .filter(|receipt| receipt.as_str() == Some("context_complete"))
            .count(),
        1,
        "context_complete must be recorded exactly once: {records:?}"
    );
    assert_eq!(
        completed_records[0]["business_receipts"],
        json!([
            "approval_decision",
            "nested_register",
            "route",
            "credential",
            "grant",
            "lease",
            "context_complete"
        ]),
        "the completed record must preserve the ordered business receipts: {records:?}"
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["phase"] == "admitted")
            .count(),
        1,
        "admission must be durable once: {records:?}"
    );
}
