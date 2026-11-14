fn preserved_peer_row<'a>(value: &'a Value, worker_id: &str) -> &'a Value {
    value["workers"]
        .as_array()
        .and_then(|workers| {
            workers
                .iter()
                .find(|worker| worker["id"].as_str() == Some(worker_id))
        })
        .unwrap_or_else(|| panic!("worker {worker_id} is missing from {value}"))
}

#[test]
fn preserved_peer_projection_keeps_displaced_worker_diagnostic_and_rejects_inactive_caller() {
    let (mut fixture, master_host, child_host, master, child) =
        lifecycle_native_pair_fixture("preserved-peer-projection", "ack_only");
    let master_id = master["worker_id"].as_str().unwrap();
    let child_id = child["worker_id"].as_str().unwrap();
    let child_session = child["runtime"]["session_id"].as_str().unwrap();
    let child_thread = child["runtime"]["native_thread_id"].as_str().unwrap();
    let scope = canonical_root(&fixture);

    lifecycle_register_task(&fixture, &master, "preserved-peer-projection-task");
    let sent = fixture
        .configured_command(
            &[
                "send",
                "--to",
                master_id,
                "--subject",
                "preserved-peer-projection-mail",
                "--content",
                "retain this mail after the current binding is displaced",
            ],
            None,
        )
        .env(
            "CODEX_APP_SERVER_SOCKET",
            child_host.endpoint().strip_prefix("unix://").unwrap(),
        )
        .env("CODEX_SESSION_ID", child_session)
        .env("CODEX_THREAD_ID", child_thread)
        .output()
        .unwrap();
    assert!(
        sent.status.success(),
        "seed mail failed: {}",
        String::from_utf8_lossy(&sent.stderr)
    );

    let before_who = fixture.run_ok(&["who"], None);
    let child_role_before = json!({
        "role": preserved_peer_row(&before_who, child_id)["role"],
        "role_brief": preserved_peer_row(&before_who, child_id)["role_brief"],
        "work_roles": preserved_peer_row(&before_who, child_id)["work_roles"],
        "transport": preserved_peer_row(&before_who, child_id)["transport"],
    });

    let before_state = fixture.current_state();
    let master_worker_before = before_state["workers"][master_id].clone();
    let master_grant_before = before_state["global"]["projects"][&scope]["master_grants"].clone();
    assert!(
        !master_grant_before.as_object().unwrap().is_empty(),
        "the displaced identity must retain an existing grant for this test"
    );
    let task_before = before_state["tasks"]["preserved-peer-projection-task"].clone();
    let message_id = before_state["msgs"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, message)| message["subject"] == "preserved-peer-projection-mail")
        .map(|(id, _)| id.clone())
        .expect("the retained mail is present before displacement");
    let message_before = before_state["msgs"][&message_id].clone();
    let credential_path = fixture
        .host_state
        .join("identities")
        .join(master_id)
        .join("identity.json");
    let credential_before = fs::read(&credential_path).unwrap();

    fixture.stop_daemon();
    let map_path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    let slot = map["slots"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|slot| slot["current"]["binding"]["agent_id"].as_str() == Some(master_id))
        .expect("the displaced worker has one current slot");
    let generation = slot["generation"].as_u64().unwrap_or(0) + 1;
    slot["previous"] = slot["current"].take();
    slot["generation"] = json!(generation);
    slot["identity_status"] = json!("unregistered");
    fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    let displaced_map = fs::read(&map_path).unwrap();
    fixture.restart_daemon();

    let context = fixture.context_provide(
        "preserved-peer-projection-context",
        &json!({
            "session_id": child_session,
            "thread_id": child_thread,
            "endpoint": child_host.endpoint(),
            "namespace": "codex_tui",
        }),
        None,
    );
    assert_eq!(
        context["result"]["outcome"], "completed",
        "the active peer context must not fail on a retained worker without current: {context}"
    );
    let snapshot = &context["result"]["snapshot"];
    assert_eq!(snapshot["identity"]["worker_id"], child_id, "{snapshot}");
    assert_eq!(snapshot["identity"]["role"], "worker", "{snapshot}");
    let context_peer = snapshot["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|peer| peer["worker_id"].as_str() == Some(master_id))
        .expect("the displaced worker remains visible in context");
    assert!(context_peer["role"].is_null(), "{context_peer}");
    assert_eq!(context_peer["work_roles"], json!([]), "{context_peer}");
    assert_eq!(context_peer["presence"], "unknown", "{context_peer}");
    assert!(context_peer["endpoint_live"].is_null(), "{context_peer}");
    assert_eq!(context_peer["activity"], "unknown", "{context_peer}");
    assert_eq!(context_peer["availability"], "unknown", "{context_peer}");

    let who = fixture.run_ok(&["who"], None);
    let displaced = preserved_peer_row(&who, master_id);
    assert_eq!(displaced["display_name"], master_id, "{displaced}");
    assert!(displaced["role"].is_null(), "{displaced}");
    assert!(displaced["role_brief"].is_null(), "{displaced}");
    assert_eq!(displaced["work_roles"], json!([]), "{displaced}");
    assert!(displaced["transport"].is_null(), "{displaced}");
    assert!(displaced["transport_view"].is_null(), "{displaced}");
    assert!(displaced["endpoint_live"].is_null(), "{displaced}");
    assert_eq!(displaced["identity_valid"], false, "{displaced}");
    assert_eq!(displaced["agent_state"], "unknown", "{displaced}");
    assert_eq!(displaced["status"], "unknown", "{displaced}");
    assert_eq!(displaced["presence"], "unknown", "{displaced}");
    let active_peer = preserved_peer_row(&who, child_id);
    assert_eq!(
        json!({
            "role": active_peer["role"],
            "role_brief": active_peer["role_brief"],
            "work_roles": active_peer["work_roles"],
            "transport": active_peer["transport"],
        }),
        child_role_before,
        "an active peer projection must not change"
    );
    assert_eq!(active_peer["identity_valid"], true, "{active_peer}");

    for args in [vec!["worker", "status", master_id], vec!["status", "--all"]] {
        let output = fixture
            .configured_command(&args, None)
            .env(
                "CODEX_APP_SERVER_SOCKET",
                child_host.endpoint().strip_prefix("unix://").unwrap(),
            )
            .env("CODEX_SESSION_ID", child_session)
            .env("CODEX_THREAD_ID", child_thread)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "collab {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let displaced = preserved_peer_row(&value, master_id);
        assert!(displaced["role"].is_null(), "{args:?}: {displaced}");
        assert!(displaced["role_brief"].is_null(), "{args:?}: {displaced}");
        assert!(displaced["transport"].is_null(), "{args:?}: {displaced}");
        assert!(
            displaced["transport_view"].is_null(),
            "{args:?}: {displaced}"
        );
        assert!(
            displaced["endpoint_live"].is_null(),
            "{args:?}: {displaced}"
        );
        assert_eq!(displaced["identity_valid"], false, "{args:?}: {displaced}");
        assert_eq!(displaced["status"], "unknown", "{args:?}: {displaced}");
    }

    let rejected = fixture
        .configured_command(
            &[
                "context",
                "--op",
                "preserved-peer-projection-inactive",
                "--query",
            ],
            None,
        )
        .env(
            "CODEX_APP_SERVER_SOCKET",
            master_host.endpoint().strip_prefix("unix://").unwrap(),
        )
        .env("CODEX_SESSION_ID", "lifecycle-master-session")
        .env("CODEX_THREAD_ID", "lifecycle-master-thread")
        .output()
        .unwrap();
    assert!(
        !rejected.status.success(),
        "a retained worker without current must not authenticate as the active caller"
    );
    let rejected_stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        rejected_stderr.contains("IDENTITY_MAP_NOT_FOUND"),
        "inactive caller must receive the strict identity error: {}",
        rejected_stderr
    );
    assert_eq!(
        fs::read(&map_path).unwrap(),
        displaced_map,
        "read-only projections and inactive-caller rejection must not rewrite the displaced control row"
    );

    let after_state = fixture.current_state();
    assert_eq!(
        after_state["workers"][master_id], master_worker_before,
        "the worker record must remain"
    );
    assert_eq!(
        after_state["global"]["projects"][&scope]["master_grants"], master_grant_before,
        "the displaced grant must remain in business state and must not transfer"
    );
    assert_eq!(
        after_state["tasks"]["preserved-peer-projection-task"], task_before,
        "the owned task must remain"
    );
    assert_eq!(
        after_state["msgs"][&message_id], message_before,
        "the retained mail must remain"
    );
    assert_eq!(
        fs::read(&credential_path).unwrap(),
        credential_before,
        "the persisted credential must remain"
    );
}
