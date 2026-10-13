#[test]
fn persisted_context_restores_without_live_collection_or_admission() {
    use std::os::unix::fs::PermissionsExt;
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("restore-offline");
    let seed = fixture.registered_context("ctxop-offline-seed", &pane);
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user assigned offline master",
        ],
        Some(&pane),
    );
    let master = master_status(&fixture, &pane);
    let binding = seed["result"]["snapshot"]["binding"].clone();
    let effects = project_register_effect_counts(&fixture.project_journal());
    let wrapper_dir = fixture.root.join("failed-display");
    fs::create_dir(&wrapper_dir).unwrap();
    let wrapper = wrapper_dir.join("tmux");
    fs::write(
        &wrapper,
        "#!/bin/sh\necho 'fixture display/capture I/O failed' >&2\nexit 71\n",
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    for step in 0..2 {
        if step == 1 {
            fixture.restart_daemon();
        }
        let op = format!("ctxop-offline-{step}");
        let output = fixture
            .configured_command(&["context", "--op", &op], Some(&pane))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    wrapper_dir.display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env(
                "CODEX_APP_SERVER_SOCKET",
                "/tmp/unavailable-adjacent-adaptor.sock",
            )
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("fixture display/capture I/O failed"),
            "actual collection failure must remain explicit"
        );
        let result = success_payload(&output);
        assert_eq!(result["result"]["outcome"], "completed", "{result}");
        assert_eq!(result["result"]["snapshot"]["binding"], binding);
        assert_eq!(
            result["result"]["snapshot"]["master"]["worker_id"],
            master["worker_id"]
        );
        assert!(result["result"]["nested_command_id"].is_null());
        assert_eq!(
            project_register_effect_counts(&fixture.project_journal()),
            effects,
            "restore must not register or admit the unavailable adjacent adaptor"
        );
        assert_eq!(master_status(&fixture, &pane), master);
    }
}

#[test]
fn single_context_master_peer_intent_overrides_and_replays_after_restart() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("single-role");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-role-seed", false);
    let other = second_pane(&fixture, "unrelated-master", pane.server_pid);
    let other_context = fixture.registered_context("ctxop-other-seed", &other);
    let other_id = other_context["result"]["snapshot"]["identity"]["worker_id"].clone();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user assigned unrelated master",
        ],
        Some(&other),
    );
    let unrelated = master_status(&fixture, &other);
    for (step, role) in ["peer", "master", "peer"].into_iter().enumerate() {
        let op = format!("ctxop-role-public-{step}");
        let approval = json!({"decision":"approved", "target_identity":fence.worker_id,
            "action":format!("restore_{role}"), "authorization":format!("user requires {role}"),
            "expected_incumbent":{"binding_id":"old-caller-fence", "endpoint_generation":999}});
        let (identity_approval, grant_approval) = if role == "master" {
            (None, Some(&approval))
        } else {
            (Some(&approval), None)
        };
        let output = run_context_cli(
            &fixture,
            Some(&pane),
            Some(&op),
            None,
            None,
            identity_approval,
            grant_approval,
            None,
            false,
        );
        let result = success_payload(&output);
        assert_eq!(result["result"]["outcome"], "completed", "{result}");
        assert!(result["result"]["committed_phases"]
            .as_array()
            .unwrap()
            .contains(&json!("grant")));
        assert_eq!(
            result["result"]["snapshot"]["identity"]["role"],
            if role == "master" { "master" } else { "worker" }
        );
        let grant = master_status(&fixture, &pane);
        if step == 0 {
            assert_eq!(grant, unrelated, "peer intent preserves unrelated master");
        } else if role == "master" {
            assert_eq!(grant["worker_id"], fence.worker_id);
            assert_eq!(grant["approval"], "user requires master");
            let demoted = fixture.context(Some("ctxop-other-demoted"), Some(&other));
            assert_eq!(demoted["result"]["snapshot"]["identity"]["role"], "worker");
            assert_eq!(
                demoted["result"]["snapshot"]["master"]["worker_id"],
                fence.worker_id
            );
            assert_ne!(other_id, fence.worker_id);
        } else {
            assert!(
                grant.is_null(),
                "target peer intent clears its own master grant"
            );
        }
        let bytes = (
            fs::read(fixture.host_journal()).unwrap(),
            fs::read(fixture.project_journal()).unwrap(),
            fs::read(&fence.identity_path).unwrap(),
        );
        let replay = run_context_cli(
            &fixture,
            Some(&pane),
            Some(&op),
            None,
            None,
            identity_approval,
            grant_approval,
            None,
            false,
        );
        assert_eq!(success_payload(&replay)["result"]["outcome"], "completed");
        assert_eq!(
            bytes,
            (
                fs::read(fixture.host_journal()).unwrap(),
                fs::read(fixture.project_journal()).unwrap(),
                fs::read(&fence.identity_path).unwrap()
            )
        );
        fixture.restart_daemon();
        assert_eq!(
            master_status(&fixture, &pane),
            grant,
            "restart retains exact durable role"
        );
        let after_restart = (
            fs::read(fixture.host_journal()).unwrap(),
            fs::read(fixture.project_journal()).unwrap(),
            fs::read(&fence.identity_path).unwrap(),
        );
        let replay = run_context_cli(
            &fixture,
            Some(&pane),
            Some(&op),
            None,
            None,
            identity_approval,
            grant_approval,
            None,
            false,
        );
        assert_eq!(success_payload(&replay)["result"]["outcome"], "completed");
        assert_eq!(
            after_restart,
            (
                fs::read(fixture.host_journal()).unwrap(),
                fs::read(fixture.project_journal()).unwrap(),
                fs::read(&fence.identity_path).unwrap(),
            ),
            "same-key replay after restart must not repeat effects"
        );
        let restored = fixture.context(Some(&format!("ctxop-role-readback-{step}")), Some(&pane));
        assert_eq!(
            restored["result"]["snapshot"]["identity"]["worker_id"],
            fence.worker_id
        );
    }
}

#[test]
fn b01_automatic_context_persists_proof_before_effect_and_redacts_capability() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-b01");
    let result = fixture.registered_automatic_context(&pane);
    let operation_id = result["operation_id"]
        .as_str()
        .expect("automatic context returns its durable operation id");
    let body = &result["result"];
    assert_eq!(body["outcome"], "completed", "{result}");
    assert_eq!(body["snapshot"]["registered"], true, "{result}");
    assert_eq!(
        body["committed_phases"],
        json!([
            "nested_register",
            "route",
            "credential",
            "lease",
            "context_complete"
        ]),
        "{result}"
    );

    let proof = fixture.proof(operation_id);
    let capability = proof["query_capability"]
        .as_str()
        .expect("private query capability exists");
    let expected_hash = format!("sha256:{:x}", Sha256::digest(capability.as_bytes()));
    let host = operation_records(&fixture.host_journal(), operation_id);
    assert_eq!(host.len(), 5, "admission plus four transitions: {host:?}");
    assert_eq!(host[0]["phase"], "admitted");
    assert_eq!(host[0]["query_capability_hash"], expected_hash);
    assert!(
        fs::metadata(fixture.proof_path(operation_id))
            .expect("proof metadata")
            .len()
            > 0,
        "the CLI must persist a query proof before the daemon can admit the operation"
    );
    for (label, path) in [
        ("host journal", fixture.host_journal()),
        ("project journal", fixture.project_journal()),
        ("host log", fixture.host_log()),
        ("host events", fixture.host_events()),
    ] {
        if let Ok(bytes) = fs::read(path) {
            assert_no_secret_text(label, &String::from_utf8_lossy(&bytes), capability);
        }
    }
}

#[test]
fn b02_same_key_retry_is_read_only_and_changed_facts_conflict() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-b02");
    let operation_id = "ctxop-v23-b02-same-key";
    let first = fixture.registered_context(operation_id, &pane);
    assert_eq!(first["result"]["outcome"], "completed", "{first}");
    let before = stable_surfaces(&fixture, &pane, operation_id);

    let retry = fixture.context(Some(operation_id), Some(&pane));
    assert_eq!(retry["result"]["outcome"], "completed", "{retry}");
    assert_eq!(stable_surfaces(&fixture, &pane, operation_id), before);

    let other = second_pane(&fixture, "b02", pane.server_pid);
    let conflict = fixture.command(&["context", "--op", operation_id], Some(&other));
    let conflict_text = text(&conflict);
    assert!(
        !conflict.status.success(),
        "changed facts must fail: {conflict_text}"
    );
    assert!(
        conflict_text.contains("IDENTITY_OPERATION_INTENT_CONFLICT")
            || conflict_text.contains("IDENTITY_FACT_CONFLICT"),
        "changed facts must produce a typed conflict: {conflict_text}"
    );
    assert_eq!(stable_surfaces(&fixture, &pane, operation_id), before);
}

#[test]
fn b03_discarded_response_recovers_same_nested_receipt_after_restart() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("collab-v23-b03");
    let operation_id = "ctxop-v23-b03-lost-response";
    let discarded = fixture.registered_context(operation_id, &pane);
    assert!(
        discarded["ok"] == true && discarded["result"]["outcome"] == "completed",
        "first public transaction failed: {discarded}"
    );
    drop(discarded); // Model a caller that committed the invocation but lost its response.

    let initial = operation_records(&fixture.host_journal(), operation_id);
    assert_eq!(initial.last().unwrap()["phase"], "completed", "{initial:?}");
    let nested = initial
        .iter()
        .find(|record| record["nested_command_id"].is_string())
        .expect("durable validating phase binds nested receipt IDs");
    let command_id = nested["nested_command_id"].as_str().unwrap().to_owned();
    let nested_operation_id = nested["nested_operation_id"].as_str().unwrap().to_owned();
    assert!(initial
        .iter()
        .filter(|row| row["nested_command_id"].is_string())
        .all(|row| {
            row["nested_command_id"] == command_id
                && row["nested_operation_id"] == nested_operation_id
        }));

    fixture.restart_daemon();
    let before = fs::read(fixture.host_journal()).unwrap();
    let queried = fixture.context_query(operation_id, Some(&pane));
    assert_eq!(
        queried["result"]["queried_operation"]["nested_command_id"],
        command_id
    );
    assert_eq!(
        queried["result"]["queried_operation"]["nested_operation_id"],
        nested_operation_id
    );
    assert_eq!(
        queried["result"]["queried_operation"]["nested_receipt"]["command_id"],
        command_id
    );
    assert_eq!(
        queried["result"]["queried_operation"]["nested_receipt"]["operation_id"],
        nested_operation_id
    );
    assert_eq!(fs::read(fixture.host_journal()).unwrap(), before);
}

#[test]
fn b04_restart_query_replays_every_durable_phase_without_mutation() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let cases = [
        ("admitted", "unknown", 1usize),
        ("validating", "unknown", 2),
        ("inner_dispatched", "unknown", 3),
        ("effect_observed", "partial", 4),
        ("completed", "completed", 5),
    ];
    for (target_phase, expected_outcome, allowed_records) in cases {
        let label = format!("collab-v23-b04-{target_phase}");
        let mut fixture = Fixture::without_tmux(&label);
        let seed_host = TestAppServer::start_with_effect_at(&fixture.root, "seed.sock",
            "b04-seed-session", "b04-seed-thread", "ack_only");
        let seed_facts = json!({"session_id":"b04-seed-session", "thread_id":"b04-seed-thread",
            "endpoint":seed_host.endpoint(), "namespace":"codex_tui"});
        let seed = fixture.context_provide("ctxop-v23-b04-seed", &seed_facts, None);
        assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
        let seed_worker = seed["result"]["snapshot"]["identity"]["worker_id"].clone();
        let seed_operation = operation_records(&fixture.host_journal(), "ctxop-v23-b04-seed");
        let seed_receipt = seed_operation.last().expect("completed seed operation");
        assert!(seed_receipt["nested_command_id"].is_string(), "initial participant binds its own command");
        assert!(seed_receipt["nested_operation_id"].is_string(), "initial participant binds its own operation");
        let project_effects_before_reuse =
            project_register_effect_counts(&fixture.project_journal());
        for index in 0..100 {
            let host_len = fs::metadata(fixture.host_journal()).unwrap().len();
            let project_len = fs::metadata(fixture.project_journal()).unwrap().len();
            if host_len > project_len + 16 * 1024 {
                break;
            }
            let operation_id = format!("ctxop-v23-b04-pad-{index:04}");
            let output = run_context_cli(&fixture, None, Some(&operation_id), None, None,
                None, None, Some(&seed_facts), false);
            let typed = json_stdout(&output);
            assert_eq!(typed["result"]["outcome"], "completed", "{typed}");
            assert_eq!(
                typed["result"]["snapshot"]["identity"]["worker_id"], seed_worker,
                "padding contexts must reuse the registered identity: {typed}"
            );
            assert_eq!(typed["result"]["snapshot"]["binding"], seed["result"]["snapshot"]["binding"]);
            if index == 0 {
                let padding_records = operation_records(&fixture.host_journal(), &operation_id);
                assert_eq!(
                    padding_records.last().unwrap()["nested_command_id"],
                    Value::Null,
                    "ordinary same-owner context must not invent or borrow a nested Register command"
                );
                assert_eq!(
                    padding_records.last().unwrap()["nested_operation_id"],
                    Value::Null,
                    "ordinary same-owner context must not invent or borrow a nested Register operation"
                );
                assert_eq!(
                    padding_records
                        .iter()
                        .filter(|record| record["phase"] == "inner_dispatched")
                        .count(),
                    1,
                    "the new outer control operation advances once without Register"
                );
                assert_eq!(
                    padding_records.last().unwrap()["business_receipts"],
                    json!([
                        "route",
                        "credential",
                        "lease",
                        "context_complete"
                    ]),
                    "the new outer operation records only read-back-proven owner receipts"
                );
            }
        }
        assert_eq!(
            project_register_effect_counts(&fixture.project_journal()),
            project_effects_before_reuse,
            "same-peer context reuse must not repeat Register, binding, or route effects"
        );
        let host_len = fs::metadata(fixture.host_journal()).unwrap().len();
        let project_len = fs::metadata(fixture.project_journal()).unwrap().len();
        assert!(
            host_len > project_len + 16 * 1024,
            "outer journal must exceed the project journal before RLIMIT_FSIZE: host={host_len} project={project_len}"
        );

        let calibration_id = "ctxop-v23-b04-calibrate";
        let calibration_host = TestAppServer::start_with_effect_at(&fixture.root, "calibration.sock",
            "b04-calibrate-session", "b04-calibrate-thread", "ack_only");
        let calibration_facts = json!({"session_id":"b04-calibrate-session", "thread_id":"b04-calibrate-thread",
            "endpoint":calibration_host.endpoint(), "namespace":"codex_tui"});
        let calibrated = fixture.context_provide(calibration_id, &calibration_facts, None);
        assert_eq!(calibrated["result"]["outcome"], "completed", "{calibrated}");
        let calibration_lengths = operation_line_lengths(&fixture.host_journal(), calibration_id);
        assert_eq!(calibration_lengths.len(), 5, "{calibration_lengths:?}");

        let target_host = TestAppServer::start_with_effect_at(&fixture.root, "target-0000.sock",
            "b04-targetxxx-session", "b04-targetxxx-thread", "ack_only");
        let target_facts = json!({"session_id":"b04-targetxxx-session", "thread_id":"b04-targetxxx-thread",
            "endpoint":target_host.endpoint(), "namespace":"codex_tui"});
        let target_id = "ctxop-v23-b04-target000";
        assert_eq!(calibration_id.len(), target_id.len());
        let journal_len = fs::metadata(fixture.host_journal()).unwrap().len();
        let file_limit = journal_len + calibration_lengths[..allowed_records].iter().sum::<u64>();
        let project_len = fs::metadata(fixture.project_journal()).unwrap().len();
        assert!(
            file_limit > project_len + 4 * 1024,
            "outer journal cap must leave room for a public project Register append: limit={file_limit} project={project_len}"
        );
        fixture.restart_daemon_with_file_limit(file_limit);

        let attempt = run_context_cli(&fixture, None, Some(target_id), None, None,
            None, None, Some(&target_facts), false);
        let typed = json_stdout(&attempt);
        if matches!(target_phase, "inner_dispatched" | "effect_observed") {
            assert_ne!(attempt.status.code(), Some(0), "{}", text(&attempt));
            assert_eq!(typed["ok"], false, "{typed}");
            assert_eq!(
                typed["result"]["owner_readback"]["error"],
                format!(
                    "IDENTITY_OPERATION_DURABILITY_FAILED: append: {}",
                    std::io::Error::from_raw_os_error(libc::EFBIG)
                ),
                "the finish error must survive the public projection: {typed}"
            );
            assert_eq!(typed["result"]["failed_phase"], target_phase, "{typed}");
            let retained = if target_phase == "inner_dispatched" {
                json!([])
            } else {
                calibrated["result"]["committed_phases"].clone()
            };
            assert_eq!(typed["result"]["committed_phases"], retained, "{typed}");
            println!(
                "B04 late OS failure {target_phase}: exit={:?} {}",
                attempt.status.code(),
                text(&attempt)
            );
        }
        assert_eq!(
            typed["result"]["outcome"],
            expected_outcome,
            "fault boundary for {target_phase}: {}",
            text(&attempt)
        );
        assert_eq!(
            fs::metadata(fixture.host_journal()).unwrap().len(),
            file_limit,
            "the daemon must reach the calibrated OS host-journal boundary for {target_phase}"
        );
        let committed = operation_records(&fixture.host_journal(), target_id);
        assert_eq!(
            committed.len(),
            allowed_records,
            "{target_phase}: {committed:?}"
        );
        assert_eq!(committed.last().unwrap()["phase"], target_phase);
        if matches!(target_phase, "inner_dispatched" | "effect_observed") {
            assert_eq!(
                typed["result"]["committed_phases"],
                committed
                    .last()
                    .unwrap()
                    .get("business_receipts")
                    .cloned()
                    .unwrap_or(json!([])),
                "the public failure retains only durable business receipts: {typed}"
            );
        }

        fixture.restart_daemon();
        let before = (
            fs::read(fixture.host_journal()).unwrap(),
            fs::read(fixture.host_routes()).unwrap_or_default(),
            fs::read(fixture.project_journal()).unwrap(),
        );
        let query = fixture.context_query(target_id, None);
        assert_eq!(
            query["result"]["queried_operation"]["phase"], target_phase,
            "replayed phase {target_phase}: {query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], expected_outcome,
            "replayed outcome for {target_phase}: {query}"
        );
        let expected_committed: Vec<&str> = match target_phase {
            "admitted" => vec![],
            "validating" => vec!["validating"],
            "inner_dispatched" => vec!["validating", "inner_dispatched"],
            "effect_observed" => vec!["validating", "inner_dispatched", "effect_observed"],
            "completed" => vec![
                "validating",
                "inner_dispatched",
                "effect_observed",
                "completed",
            ],
            _ => unreachable!(),
        };
        assert_eq!(
            query["result"]["queried_operation"]["committed_phases"],
            json!(expected_committed)
        );
        if target_phase == "validating" {
            assert_eq!(
                query["result"]["queried_operation"]["nested_receipt"]["state"], "unavailable",
                "incomplete Register replay must stay queryable as degraded evidence: {query}"
            );
            assert!(
                query["result"]["queried_operation"]["nested_receipt"]["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("PROJECT_OWNER_READBACK_UNAVAILABLE")),
                "degraded query preserves its owner replay failure: {query}"
            );
        } else if target_phase == "inner_dispatched" {
            assert_eq!(
                query["result"]["queried_operation"]["nested_receipt"]["command_id"],
                query["result"]["queried_operation"]["nested_command_id"],
                "replayed Register receipt is tied to the exact outer command ID: {query}"
            );
            assert_eq!(
                query["result"]["queried_operation"]["nested_receipt"]["operation_id"],
                query["result"]["queried_operation"]["nested_operation_id"],
                "replayed Register receipt is tied to the exact outer operation ID: {query}"
            );
        }
        assert_eq!(
            (
                fs::read(fixture.host_journal()).unwrap(),
                fs::read(fixture.host_routes()).unwrap_or_default(),
                fs::read(fixture.project_journal()).unwrap(),
            ),
            before,
            "operation query must not mutate either journal or the host route projection"
        );
        if target_phase == "validating" {
            fixture.stop_degraded_daemon();
        } else if matches!(target_phase, "inner_dispatched" | "effect_observed") {
            let target_surfaces = || {
                let output = fixture.configured_command(&["notify", "status"], None)
                    .env("CODEX_APP_SERVER_SOCKET", target_host.endpoint().strip_prefix("unix://").unwrap())
                    .env("CODEX_SESSION_ID", "b04-targetxxx-session")
                    .env("CODEX_THREAD_ID", "b04-targetxxx-thread")
                    .output().unwrap();
                assert!(output.status.success(), "{}", text(&output));
                (operation_records(&fixture.host_journal(), target_id),
                    fs::read(fixture.host_routes()).unwrap_or_default(),
                    project_register_effect_counts(&fixture.project_journal()), json_stdout(&output))
            };
            let before = target_surfaces();
            let journal_bytes = (
                fs::read(fixture.host_journal()).unwrap(),
                fs::read(fixture.project_journal()).unwrap(),
            );
            let retry = run_context_cli(&fixture, None, Some(target_id), None, None,
                None, None, Some(&target_facts), false);
            let replay = json_stdout(&retry);
            assert_ne!(retry.status.code(), Some(0), "{}", text(&retry));
            assert_eq!(replay["result"]["outcome"], expected_outcome, "{replay}");
            assert_eq!(
                replay["result"]["committed_phases"], typed["result"]["committed_phases"],
                "{replay}"
            );
            assert_eq!(
                target_surfaces(),
                before,
                "same explicit key must only read the retained projection"
            );
            assert_eq!(
                (
                    fs::read(fixture.host_journal()).unwrap(),
                    fs::read(fixture.project_journal()).unwrap()
                ),
                journal_bytes,
                "same-key invocation must append nothing to either journal"
            );
            let effects = project_register_effect_counts(&fixture.project_journal());
            let fresh = success_payload(&run_context_cli(&fixture, None, None, None, None,
                None, None, Some(&target_facts), false));
            assert_eq!(fresh["result"]["outcome"], "completed", "{fresh}");
            assert_ne!(fresh["operation_id"], target_id, "{fresh}");
            assert!(fresh["result"]["requires"]["approval"].is_null(), "{fresh}");
            assert_eq!(
                project_register_effect_counts(&fixture.project_journal()),
                effects,
                "fresh invocation reuses the committed identity without repeating Register"
            );
            println!(
                "B04 readonly retry {target_phase}: exit={:?} {}; fresh default: {fresh}",
                retry.status.code(),
                text(&retry)
            );
        }
    }
}

#[test]
fn b05_query_remains_available_when_real_project_journal_is_unavailable() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("collab-v23-b05");
    let operation_id = "ctxop-v23-b05-degraded";
    let completed = fixture.registered_context(operation_id, &pane);
    assert_eq!(completed["result"]["outcome"], "completed", "{completed}");
    let project_bytes = fs::read(fixture.project_journal()).expect("real journal exists");
    let host_before = fs::read(fixture.host_journal()).unwrap();

    fixture.stop_daemon();
    fixture.hide_project_journal();
    assert!(fixture.project_journal().is_dir());
    fixture.restart_daemon();
    let query = fixture.context_query(operation_id, Some(&pane));
    assert_eq!(query["result"]["outcome"], "completed", "{query}");
    assert_eq!(
        query["result"]["queried_operation"]["operation_id"],
        operation_id
    );
    assert_eq!(fs::read(fixture.host_journal()).unwrap(), host_before);
    assert!(fixture.project_journal().is_dir());

    fixture.stop_degraded_daemon();
    fixture.restore_project_journal();
    assert_eq!(fs::read(fixture.project_journal()).unwrap(), project_bytes);
}

#[test]
fn b06_mcp_query_uses_retained_capability_and_rejects_wrong_one_without_leaks() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-b06");
    let operation_id = "ctxop-v23-b06-capability";
    let completed = fixture.registered_context(operation_id, &pane);
    assert_eq!(completed["result"]["outcome"], "completed", "{completed}");
    let proof = fixture.proof(operation_id);
    let capability = proof["query_capability"].as_str().unwrap().to_owned();
    let before = stable_surfaces(&fixture, &pane, operation_id);
    let operation_before = operation_records(&fixture.project_journal(), operation_id);

    let mut mcp = Mcp::start(&fixture, Some(&pane));
    let result = mcp.call_context(6, json!({"operation_id": operation_id, "query": true}));
    assert_eq!(
        result["isError"], false,
        "correct capability query failed: {result}"
    );
    let mcp_text = result["content"][0]["text"]
        .as_str()
        .expect("MCP query includes typed result text");
    let mcp_payload: Value = serde_json::from_str(mcp_text).expect("MCP typed result is JSON");
    assert_eq!(
        mcp_payload["result"]["queried_operation"]["operation_id"],
        operation_id
    );
    assert_no_secret_text("MCP result", mcp_text, &capability);
    mcp.finish();

    fixture.set_proof_capability(operation_id, "wrong-capability-test-value");
    let denied = fixture.command(&["context", "--op", operation_id, "--query"], Some(&pane));
    let denied_text = text(&denied);
    assert!(
        !denied.status.success(),
        "wrong capability must be denied: {denied_text}"
    );
    assert!(
        denied_text.contains("IDENTITY_OPERATION_QUERY_DENIED"),
        "wrong capability must produce the typed denial: {denied_text}"
    );
    assert!(!denied_text.contains("wrong-capability-test-value"));
    fixture.set_proof_capability(operation_id, &capability);
    let after = stable_surfaces(&fixture, &pane, operation_id);
    assert_eq!(
        after.0, before.0,
        "query changed the host operation journal"
    );
    assert_eq!(after.1, before.1, "query changed host route state");
    assert_eq!(
        operation_records(&fixture.project_journal(), operation_id),
        operation_before,
        "query changed this operation's durable project journal records"
    );
    assert_eq!(after.3, before.3, "query changed notification state");
    for path in [
        fixture.host_journal(),
        fixture.project_journal(),
        fixture.host_log(),
        fixture.host_events(),
    ] {
        if let Ok(bytes) = fs::read(path) {
            assert_no_secret_text(
                "persisted surface",
                &String::from_utf8_lossy(&bytes),
                &capability,
            );
        }
    }
}

#[test]
fn b07_missing_facts_are_exact_and_fresh_supplement_registers_through_public_cli() {
    let fixture = Fixture::without_tmux("collab-v23-b07");
    let operation_id = "ctxop-v23-b07-supplement";
    let session_id = format!("test-session-{}", std::process::id());
    let thread_id = format!("test-thread-{}", std::process::id());
    let appserver = TestAppServer::start(&fixture.root, &session_id, &thread_id);

    let missing_output = fixture.command(&["context", "--op", operation_id], None);
    assert_eq!(missing_output.status.code(), Some(2), "{missing_output:?}");
    let missing = json_stdout(&missing_output);
    assert_eq!(missing["result"]["outcome"], "missing_facts", "{missing}");
    assert_eq!(missing["result"]["requires"]["kind"], "identity_facts");
    assert_eq!(
        missing["result"]["requires"]["fields"],
        json!(["session_id", "thread_id", "endpoint"])
    );
    assert_eq!(
        missing["result"]["requires"]["sources"]["session_id"],
        "Current runtime session identifier"
    );
    assert_eq!(
        missing["result"]["requires"]["sources"]["thread_id"],
        "Current native thread identifier"
    );
    assert_eq!(
        missing["result"]["requires"]["sources"]["endpoint"],
        "Current native AppServer unix socket endpoint"
    );
    assert_eq!(
        missing["result"]["requires"]["repair_invocation"],
        "collab context --provide '<JSON containing required_fields>'"
    );
    assert!(operation_records(&fixture.host_journal(), operation_id).is_empty());
    assert!(operation_records(&fixture.project_journal(), operation_id).is_empty());

    let facts = json!({
        "session_id": session_id,
        "thread_id": thread_id,
        "endpoint": appserver.endpoint(),
        "namespace": "codex_tui"
    });
    let completed = fixture.context_provide(operation_id, &facts, None);
    assert_eq!(
        completed["result"]["invocation"], "supplement",
        "{completed}"
    );
    assert_eq!(completed["result"]["outcome"], "completed", "{completed}");
    assert_eq!(
        completed["result"]["snapshot"]["registered"], true,
        "{completed}"
    );
    let queried = fixture.context_query(operation_id, None);
    assert_eq!(
        queried["result"]["queried_operation"]["phase"], "completed",
        "{queried}"
    );
    drop(appserver);
}

#[test]
fn b08_os_append_failure_is_typed_and_never_projects_durable_completion() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("collab-v23-b08");
    let seed = fixture.registered_context("ctxop-v23-b08-seed", &pane);
    assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
    let journal_path = fixture.host_journal();
    let valid_prefix = fs::metadata(&journal_path).unwrap().len();
    fixture.restart_daemon_with_file_limit(valid_prefix);

    let failed_id = "ctxop-v23-b08-failed";
    let failed = fixture.command(&["context", "--op", failed_id], Some(&pane));
    let failed_text = text(&failed);
    assert_ne!(failed.status.code(), Some(0), "{failed_text}");
    println!(
        "B08 admission OS failure: exit={:?} {failed_text}",
        failed.status.code()
    );
    assert!(
        failed_text.contains("IDENTITY_OPERATION_DURABILITY_FAILED"),
        "the public request must surface the real append failure: {failed_text}"
    );
    assert!(operation_records(&journal_path, failed_id).is_empty());

    let query = fixture.command(&["context", "--op", failed_id, "--query"], Some(&pane));
    let query_text = text(&query);
    assert!(
        !query.status.success(),
        "unknown operation must not appear successful: {query_text}"
    );
    assert!(
        query_text.contains("IDENTITY_OPERATION_UNKNOWN"),
        "query must return the typed unknown state: {query_text}"
    );
    assert!(operation_records(&journal_path, failed_id).is_empty());
    assert!(
        wait_until(Duration::from_secs(2), || fixture
            .host_state
            .join("server.sock")
            .exists()),
        "fixture daemon remains the owner after append failure"
    );
}

#[test]
fn c03_unapproved_unique_pane_stale_recovery_repairs_without_approval() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c03");
    let operation_id = "ctxop-v23-c03-unapproved";
    let fence = seed_stale_identity(&fixture, &pane, operation_id, false);
    let host_before = fs::read(fixture.host_journal()).expect("host journal before recovery");
    let recovery_id = "ctxop-v23-c03-recovery";

    let automatic = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        None,
        None,
        None,
        false,
    );
    let automatic_payload = success_payload(&automatic);
    let automatic_text = text(&automatic);
    assert!(
        automatic.status.success(),
        "unique same-pane stale credential must auto-repair: {automatic_text}"
    );
    assert!(
        !automatic_text.contains("IDENTITY_APPROVAL_REQUIRED"),
        "automatic pane recovery must not request approval: {automatic_text}"
    );
    assert_eq!(
        automatic_payload["result"]["outcome"], "completed",
        "{automatic_payload}"
    );
    assert!(
        !automatic_text.contains("stale-"),
        "approval result must not leak the stale credential: {automatic_text}"
    );
    assert_eq!(
        automatic_payload["result"]["snapshot"]["identity"]["worker_id"], fence.worker_id,
        "{automatic_payload}"
    );
    assert!(fs::read(fixture.host_journal()).unwrap().len() > host_before.len());
    assert_eq!(
        automatic_payload["result"]["snapshot"]["binding"]["binding_id"],
        fence.binding_id
    );
    assert_eq!(
        automatic_payload["result"]["snapshot"]["binding"]["endpoint_generation"],
        fence.endpoint_generation
    );
    assert_eq!(automatic_payload["result"]["snapshot"]["registered"], true);
    assert_ne!(
        fs::read(&fence.identity_path).expect("stale cache replaced"),
        fence.stale_identity_bytes,
        "the daemon must replace the stale local credential"
    );
}

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
