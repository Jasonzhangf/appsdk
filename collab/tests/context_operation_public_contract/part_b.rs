#[test]
fn public_context_bootstraps_an_empty_resident_route() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("resident-route-empty");
    let root = canonical_root(&fixture);
    let output = fixture.command(
        &["context", "--op", "ctxop-resident-route-empty"],
        Some(&pane),
    );
    assert!(
        output.status.success(),
        "fresh public context must bootstrap the resident route: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let records = fs::read_to_string(fixture.host_routes())
        .expect("fresh public context must publish a host route journal");
    let records: Vec<Value> = records
        .lines()
        .map(|line| serde_json::from_str(line).expect("route journal record is JSON"))
        .collect();
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["op"], "register");
    assert_eq!(records[0]["project_scope"], root);
    assert_eq!(records[0]["canonical_root"], root);
    let storage = std::path::Path::new(records[0]["storage_root"].as_str().unwrap());
    assert!(storage.starts_with(std::fs::canonicalize(&fixture.host_state).unwrap().join("projects")));
    assert!(storage.join(PROJECT_JOURNAL).is_file());
}

#[test]
fn public_context_rebuilds_a_missing_resident_route_after_restart() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("resident-route-bootstrap");
    let seed = fixture.registered_context("ctxop-resident-route-seed", &pane);
    let worker_id = seed["result"]["snapshot"]["identity"]["worker_id"]
        .clone();
    let root = canonical_root(&fixture);

    fixture.stop_daemon();
    fs::remove_file(fixture.host_routes()).expect("remove isolated route journal");
    fixture.restart_daemon();

    let recovered = success_payload(&fixture.command(
        &["context", "--op", "ctxop-resident-route-recover"],
        Some(&pane),
    ));
    assert_eq!(
        recovered["result"]["snapshot"]["identity"]["worker_id"],
        worker_id
    );
    let records = fs::read_to_string(fixture.host_routes())
        .expect("public context must recreate the host route journal");
    let records: Vec<Value> = records
        .lines()
        .map(|line| serde_json::from_str(line).expect("route journal record is JSON"))
        .collect();
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["op"], "register");
    assert_eq!(records[0]["project_scope"], root);
    assert_eq!(records[0]["canonical_root"], root);
    let storage = std::path::Path::new(records[0]["storage_root"].as_str().unwrap());
    assert!(storage.starts_with(std::fs::canonicalize(&fixture.host_state).unwrap().join("projects")));
    assert!(storage.join(PROJECT_JOURNAL).is_file());

    fixture.restart_daemon();
    let replay = success_payload(&fixture.command(
        &["context", "--op", "ctxop-resident-route-recover"],
        Some(&pane),
    ));
    assert_eq!(
        replay["result"]["snapshot"]["identity"]["worker_id"],
        worker_id
    );
}

#[test]
fn persisted_context_uses_daemon_observation_without_adjacent_admission() {
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
            !String::from_utf8_lossy(&output.stderr).contains("fixture display/capture I/O failed"),
            "caller PATH must not replace the daemon's placement observer"
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
    let mut work_name = Value::Null;
    let mut task_baseline = Value::Null;
    let mut message_baseline = Value::Null;
    for (step, role) in ["peer", "master", "peer"].into_iter().enumerate() {
        let op = format!("ctxop-role-public-{step}");
        let approval = json!({"decision":"approved", "target_identity":fence.worker_id,
            "action":format!("restore_{role}"), "authorization":format!("user requires {role}"),
            "replace_confirmed": role == "master",
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
            .contains(&json!("grant")), "role step {step} must retain its authority receipt: {result}");
        assert_eq!(result["result"]["snapshot"]["identity"]["worker_id"], fence.worker_id);
        if step == 0 {
            work_name = result["result"]["snapshot"]["identity"]["display_name"].clone();
            assert_eq!(work_name, result["result"]["snapshot"]["name_resolution"]["name"]);
            assert!(work_name.is_string());
            fixture.run_ok(&["task", "register", "role-preserved-task", "--next", "retain responsibility"], Some(&pane));
            fixture.run_ok(&["sendmessage", "--to", &fence.worker_id, "--subject", "role preserved mailbox", "--delivery", "queued", "retain unread message"], Some(&pane));
            task_baseline = role_consumer_task_facts(&fixture, &pane);
            message_baseline = read_lines(&fixture.project_journal()).into_iter()
                .find(|event| event["ev"] == "Sent" && event["msg"]["subject"] == "role preserved mailbox")
                .expect("queued compatibility message has a durable ID")["msg"].clone();
        } else {
            assert_eq!(role_consumer_task_facts(&fixture, &pane), task_baseline);
            let message = result["result"]["snapshot"]["inbox"]["messages"].as_array().unwrap().iter()
                .find(|message| message["id"] == message_baseline["id"])
                .expect("worker-addressed unread message remains with the explicit worker");
            assert_eq!(message["body"], message_baseline["body"]);
            assert_eq!(message["from"], message_baseline["from"]);
        }
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
            assert_eq!(demoted["result"]["snapshot"]["identity"]["display_name"], work_name);
        } else {
            assert!(
                grant.is_null(),
                "target peer intent clears its own master grant"
            );
            assert_ne!(result["result"]["snapshot"]["identity"]["display_name"], work_name);
        }
        let task = fixture.run_ok(&["task", "status", "role-preserved-task"], Some(&pane));
        let expected_owner = if step == 0 { json!(fence.worker_id) } else { other_id.clone() };
        assert_eq!(task["owner"], expected_owner, "work stays with {work_name}: {task}");
        let occupancy = read_lines(&fixture.project_journal()).into_iter().rev()
            .find(|event| event["ev"] == "NameResolutionCommitted" && event["occupancy"]["name"] == work_name)
            .expect("work name has typed occupancy");
        assert_eq!(occupancy["occupancy"]["worker_id"], expected_owner);
        assert_eq!(role_consumer_task_facts(&fixture, &pane), task_baseline);
        let messages = read_lines(&fixture.project_journal());
        assert_eq!(messages.iter().find(|event| event["ev"] == "Sent" && event["msg"]["id"] == message_baseline["id"]).unwrap()["msg"], message_baseline);
        assert!(!messages.iter().any(|event| matches!(event["ev"].as_str(), Some("ReceiveCommitted" | "Delivered" | "Acked" | "Superseded"))
            && event["ids"].as_array().is_some_and(|ids| ids.contains(&message_baseline["id"]))), "context must not consume the worker-addressed message");
        let bytes = role_consumer_replay_facts(&fixture, &fence.identity_path);
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
            role_consumer_replay_facts(&fixture, &fence.identity_path)
        );
        fixture.restart_daemon();
        assert_eq!(
            master_status(&fixture, &pane),
            grant,
            "restart retains exact durable role"
        );
        let after_restart = role_consumer_replay_facts(&fixture, &fence.identity_path);
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
            role_consumer_replay_facts(&fixture, &fence.identity_path),
            "same-key replay after restart must not repeat effects"
        );
        let restored = fixture.context(Some(&format!("ctxop-role-readback-{step}")), Some(&pane));
        assert_eq!(
            restored["result"]["snapshot"]["identity"]["worker_id"],
            fence.worker_id
        );
        assert_eq!(role_consumer_task_facts(&fixture, &pane), task_baseline);
        assert_eq!(fixture.run_ok(&["task", "status", "role-preserved-task"], Some(&pane))["owner"], expected_owner);
    }
}

// Ignore only volatile status projection fields; retain task ID, state, next step,
// evidence and resource/merge/cleanup facts across name and role changes.
fn role_consumer_task_facts(fixture: &Fixture, pane: &Pane) -> Value {
    let mut task = fixture.run_ok(&["task", "status", "role-preserved-task"], Some(pane));
    let fields = task.as_object_mut().unwrap();
    fields.remove("owner"); // Checked separately against the current typed name occupancy.
    fields.remove("updated_at");
    fields.remove("keepalive");
    task
}

fn role_consumer_replay_facts(fixture: &Fixture, identity_path: &Path) -> (Vec<u8>, Vec<Value>, Vec<u8>) {
    let events = read_lines(&fixture.project_journal());
    // Timer-owned presence adjudication and reflow notices are identified by
    // typed control references, never by message body or metadata.
    let timer_messages: Vec<String> = events.iter().filter_map(|event| {
        if event["ev"] == "NameRecoveryUpdated"
            && event["recovery"]["request"]["invocation"] == "presence_adjudication"
        {
            Some(format!("name-adjudication-{}", event["recovery"]["operation_id"].as_str().unwrap()))
        } else if event["ev"] == "TaskAssignmentUpdated" {
            event["assignment"]["reflow"]["message_id"].as_str().map(str::to_owned)
        } else { None }
    }).collect();
    let is_timer_message = |id: &Value| id.as_str().is_some_and(|id| timer_messages.iter().any(|known| known == id));
    let controls = events.into_iter().filter(|event| {
        let timer_effect = match event["ev"].as_str().unwrap() {
            "GlobalRuntimeBindingLedgerClassified" | "GlobalLedgerScanReceiptRecorded"
            | "AgentPresenceObserved" | "AgentActivityObserved" => true,
            "NameRecoveryUpdated" => event["recovery"]["request"]["invocation"] == "presence_adjudication"
                && matches!(event["recovery"]["progress"].as_str(), Some("pending" | "adjudication_requested")),
            "Sent" => is_timer_message(&event["msg"]["id"]) && event["msg"]["from"] == "daemon",
            "DeliveryMode" => is_timer_message(&event["msg_id"]),
            "WakeBound" | "NotificationDeliveryAccepted" | "NotificationDeliveryFailed" => is_timer_message(&event["message_id"]),
            "WakeAttempted" => event["ids"].as_array().is_some_and(|ids| !ids.is_empty() && ids.iter().all(&is_timer_message)),
            _ => false,
        };
        !timer_effect
    }).collect();
    // All other events remain exact, including NameResolutionCommitted,
    // TaskAssignmentUpdated, TaskUpdated, BoardDetailsChanged and ReceiveCommitted.
    (fs::read(fixture.host_journal()).unwrap(), controls, fs::read(identity_path).unwrap())
}

fn same_session_pane(fixture: &Fixture, label: &str, server_pid: u32, base: &Pane) -> Pane {
    let session_id = String::from_utf8(
        tmux(
            &fixture.tmux_socket,
            &["display-message", "-p", "-t", &base.pane_id, "#{session_id}"],
        )
        .stdout,
    )
    .expect("tmux session id is UTF-8")
    .trim()
    .to_owned();
    let output = tmux(
        &fixture.tmux_socket,
        &[
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            &session_id,
            "sleep 600",
        ],
    );
    let pane_id = String::from_utf8(output.stdout)
        .expect("same-session pane id is UTF-8")
        .trim()
        .to_owned();
    Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: format!("session-{label}-{pane_id}"),
        thread_anchor: format!("thread-{label}-{pane_id}"),
    }
}

fn registered_context_in(
    fixture: &Fixture,
    project_root: &Path,
    operation_id: &str,
    pane: &Pane,
) -> Value {
    let done = fixture.root.join(format!("{operation_id}.participant-done"));
    let result_path = fixture
        .root
        .join(format!("{operation_id}.participant-result"));
    let script = format!(
        "cd {}\n\"$COLLAB_BINARY\" context --op {} >{} 2>\"$PANE_STDERR\"\nprintf done >{}\nsleep 600\n",
        shell_quote(&project_root.to_string_lossy()),
        shell_quote(operation_id),
        shell_quote(&result_path.to_string_lossy()),
        shell_quote(&done.to_string_lossy())
    );
    let consumer = fixture.run_in_pane(pane, operation_id, &script);
    assert!(
        wait_until(Duration::from_secs(15), || done.exists()),
        "external project participant did not complete: {} {}",
        consumer.output(),
        std::fs::read_to_string(&result_path).unwrap_or_default()
    );
    let result: Value = serde_json::from_slice(
        &std::fs::read(result_path).expect("read external participant result"),
    )
    .expect("external participant emits JSON");
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["result"]["outcome"], "completed", "{result}");
    result
}

fn context_with_agent_session(
    fixture: &Fixture,
    project_root: &Path,
    operation_id: &str,
    pane: &Pane,
) -> std::process::Output {
    current_pane_command(fixture, project_root, operation_id, pane, &["context", "--op", operation_id])
}

fn current_pane_command(
    fixture: &Fixture, project_root: &Path, label: &str, pane: &Pane, args: &[&str],
) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    let done = fixture.root.join(format!("{label}.exit"));
    let result = fixture.root.join(format!("{label}.result"));
    if done.exists() { fs::remove_file(&done).unwrap(); }
    let command = args.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>().join(" ");
    let script = format!(
        "cd {}\nunset CODEX_THREAD_ID\n\"$COLLAB_BINARY\" {} >{} 2>\"$PANE_STDERR\"\nprintf '%s' \"$?\" >{}\nsleep 600\n",
        shell_quote(&project_root.to_string_lossy()), command,
        shell_quote(&result.to_string_lossy()), shell_quote(&done.to_string_lossy()),
    );
    let consumer = fixture.run_in_pane(pane, label, &script);
    assert!(wait_until(Duration::from_secs(15), || done.exists()), "pane command did not finish: {}", consumer.output());
    let code: i32 = fs::read_to_string(done).unwrap().parse().unwrap();
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: fs::read(result).unwrap(), stderr: fs::read(consumer.stderr).unwrap_or_default(),
    }
}

fn identity_tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, entries: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(path).expect("read isolated identity tree") {
            let entry = entry.expect("read isolated identity entry");
            let entry_path = entry.path();
            if entry.file_type().expect("identity entry type").is_dir() {
                visit(root, &entry_path, entries);
            } else {
                entries.push((
                    entry_path
                        .strip_prefix(root)
                        .expect("identity entry under root")
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(&entry_path).expect("read isolated identity entry"),
                ));
            }
        }
    }

    let root = root.join("identities");
    let mut entries = Vec::new();
    visit(&root, &root, &mut entries);
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn seed_shared_session_identities(fixture: &Fixture, pane: &Pane) {
    let facts = tmux_identity_facts(fixture, pane);
    let endpoint = facts["tmux"]["endpoint"].clone();
    for suffix in ["ambiguous-a", "ambiguous-b"] {
        let worker_id = format!("codex-session-fixture-{suffix}");
        let identity_path = fixture
            .host_state
            .join("identities")
            .join(&worker_id)
            .join("identity.json");
        fs::create_dir_all(identity_path.parent().unwrap())
            .expect("create isolated fixture identity");
        fs::write(
            &identity_path,
            serde_json::to_vec_pretty(&json!({
                "worker_id": worker_id,
                "token": format!("fixture-token-{suffix}"),
                "project_scope": canonical_root(fixture),
                "transport": {
                    "kind": "tmux",
                    "endpoint": fixture.tmux_socket.to_string_lossy(),
                    "namespace": endpoint["tmux_session_id"],
                    "tmux_endpoint": endpoint,
                    "capabilities": ["send_message_to_pane", "probe_pane"],
                    "self_check": "isolated ambiguity fixture"
                }
            }))
            .expect("serialize isolated fixture identity"),
        )
        .expect("write isolated fixture identity");
    }
}

#[test]
fn public_context_prefers_current_project_for_a_shared_tmux_session() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("shared-tmux-current");
    let current = fixture.registered_context("shared-tmux-current-seed", &pane);
    let current_id = current["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("current project identity")
        .to_owned();

    let foreign_root = fixture.root.join("foreign-project");
    seed_baseline(&foreign_root);
    let foreign_pane = same_session_pane(
        &fixture,
        "shared-tmux-foreign",
        pane.server_pid,
        &pane,
    );
    let foreign = registered_context_in(
        &fixture,
        &foreign_root,
        "shared-tmux-foreign-seed",
        &foreign_pane,
    );
    let foreign_id = foreign["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("foreign project identity")
        .to_owned();
    assert_ne!(current_id, foreign_id);
    let task = current_pane_command(&fixture, &foreign_root, "foreign-task-create", &foreign_pane,
        &["task", "register", "foreign-preserved-task", "--next", "keep"]);
    assert!(
        task.status.success(),
        "foreign task setup failed: {}",
        String::from_utf8_lossy(&task.stderr)
    );

    let recovered = context_with_agent_session(
        &fixture,
        &fixture.root,
        "shared-tmux-current-restore",
        &pane,
    );
    let recovered = success_payload(&recovered);
    assert_eq!(
        recovered["result"]["snapshot"]["identity"]["worker_id"],
        current_id,
        "{recovered}"
    );

    let foreign_restore = context_with_agent_session(&fixture, &foreign_root, "shared-tmux-foreign-readback", &foreign_pane);
    let foreign_restore = success_payload(&foreign_restore);
    assert_eq!(
        foreign_restore["result"]["snapshot"]["identity"]["worker_id"],
        foreign_id,
        "{foreign_restore}"
    );
    let status = current_pane_command(&fixture, &foreign_root, "foreign-task-status", &foreign_pane,
        &["task", "status", "foreign-preserved-task"]);
    assert!(
        status.status.success(),
        "foreign task readback failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status = json_stdout(&status);
    assert_eq!(status["owner"], foreign_id, "{status}");
}

#[test]
fn public_context_restores_current_session_with_historical_identity_duplicates() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("shared-tmux-ambiguous");
    let current = fixture.registered_context("shared-agent-current-seed", &pane);
    seed_shared_session_identities(&fixture, &pane);
    let identities_before = identity_tree(&fixture.host_state);

    let output = context_with_agent_session(
        &fixture,
        &fixture.root,
        "shared-tmux-ambiguous-restore",
        &pane,
    );
    let payload = success_payload(&output);
    assert_eq!(payload["result"]["outcome"], "completed", "{payload}");
    let snapshot = &payload["result"]["snapshot"];
    assert_eq!(snapshot["registered"], true);
    assert!(snapshot["master"].is_null(), "history must not grant master authority");
    let worker = snapshot["identity"]["worker_id"].as_str().unwrap();
    assert_eq!(
        worker,
        current["result"]["snapshot"]["identity"]["worker_id"].as_str().unwrap(),
        "current agent session must win over historical tmux identity duplicates"
    );
    let selected_path = format!("{worker}/identity.json");
    let identities_after = identity_tree(&fixture.host_state);
    assert_eq!(identities_after.len(), identities_before.len());
    for (path, before) in identities_before {
        let after = &identities_after.iter().find(|(candidate, _)| candidate == &path).unwrap().1;
        if path == selected_path {
            let before: Value = serde_json::from_slice(&before).unwrap();
            let after: Value = serde_json::from_slice(after).unwrap();
            assert_eq!(after["token"], before["token"]);
        } else {
            assert_eq!(after, &before, "unrelated historical identity must remain intact");
        }
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
        assert!(seed_receipt["nested_command_id"].is_string(), "typed initial registration binds its own command");
        assert!(seed_receipt["nested_operation_id"].is_string(), "typed initial registration binds its own operation");
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
        if target_phase != "validating" {
            // Startup classification is an independent writer. Observe its
            // complete batch before measuring the query's byte-level purity.
            assert!(wait_until(Duration::from_secs(10), || {
                let events = read_lines(&fixture.project_journal());
                let bindings: Vec<_> = events.iter()
                    .filter(|row| row["ev"] == "GlobalCurrentThreadRouteSet")
                    .map(|row| &row["binding"]).collect();
                bindings.len() >= 2 && bindings.iter().all(|binding| {
                    let classification = events.iter().enumerate().rev().find(|(_, row)| {
                        row["ev"] == "GlobalRuntimeBindingLedgerClassified"
                            && ["project_scope", "app_scope_id", "binding_id"].iter()
                                .all(|key| row["record"][key] == binding[key])
                    });
                    classification.is_some_and(|(index, row)| {
                        let record = &row["record"];
                        record["endpoint_generation"] == binding["endpoint_generation"]
                            && record["state"] == "Live" && record["probe_state"] == "Live"
                            && events[index + 1..].iter().any(|row| {
                                row["ev"] == "GlobalLedgerScanReceiptRecorded"
                                    && row["receipt"]["scanned_ms"] == record["classified_ms"]
                                    && row["receipt"]["blocked"] == 0
                            })
                    })
                })
            }), "all committed bindings must finish startup classification before query: {target_phase}");
            println!("B04 {target_phase}: resident ledger converged before query");
        }
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

include!("part_b_tail.rs");
