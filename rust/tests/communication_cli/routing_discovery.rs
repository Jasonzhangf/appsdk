use super::*;

#[test]
fn routing_requires_explicit_master_and_respects_scope_boundaries() {
    let root = temp_root("routing");
    register_scope(
        &root,
        "scope-a",
        "app-a",
        "/project",
        &["master-a", "peer-a", "peer-b", "child-a"],
    );
    register_scope(
        &root,
        "scope-b",
        "app-b",
        "/project",
        &["master-b", "peer-c"],
    );
    register_agent(&root, "scope-a", "master-a", "master-a", "master", None);
    register_agent(&root, "scope-a", "peer-a", "peer-a", "peer", None);
    register_agent(&root, "scope-a", "peer-b", "peer-b", "peer", None);
    register_agent(
        &root,
        "scope-a",
        "child-a",
        "child-a",
        "subagent",
        Some(json!({ "scopeId": "scope-a", "sessionId": "peer-a" })),
    );
    register_agent(&root, "scope-b", "master-b", "master-b", "master", None);
    register_agent(&root, "scope-b", "peer-c", "peer-c", "peer", None);

    let local_peer = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-a" },
                "to": { "scopeId": "scope-a", "sessionId": "peer-b" },
                "title": "local peer",
                "priority": "p2",
                "body": "same appserver and project"
            }
        }),
    );
    assert_eq!(local_peer["route"]["mode"], "same-scope-peer");
    assert_eq!(local_peer["message"]["state"], "accepted");

    let bound_child = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-a" },
                "to": { "scopeId": "scope-a", "sessionId": "child-a" },
                "title": "bound child",
                "priority": "p2",
                "body": "parent scoped"
            }
        }),
    );
    assert_eq!(bound_child["route"]["mode"], "same-scope-parent");

    let cross_master = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-b", "sessionId": "master-b" },
                "title": "cross master",
                "priority": "p1",
                "body": "cross appserver same project"
            }
        }),
    );
    assert_eq!(cross_master["route"]["mode"], "cross-scope-master");

    let peer_cross = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-a" },
                "to": { "scopeId": "scope-b", "sessionId": "master-b" },
                "title": "forbidden",
                "priority": "p2",
                "body": "peer cannot cross scope"
            }
        }),
    );
    assert!(
        peer_cross.contains("cross_scope_master_required"),
        "{peer_cross}"
    );

    let unbound_child = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-b" },
                "to": { "scopeId": "scope-a", "sessionId": "child-a" },
                "title": "forbidden child",
                "priority": "p2",
                "body": "wrong parent"
            }
        }),
    );
    assert!(
        unbound_child.contains("subagent_parent_required"),
        "{unbound_child}"
    );

    let auto_role = call_error(
        &root,
        json!({
            "op": "register_agent",
            "agent": {
                "scopeId": "scope-a",
                "sessionId": "auto",
                "agentId": "auto",
                "role": "auto"
            }
        }),
    );
    assert!(auto_role.contains("role_auto_forbidden"), "{auto_role}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cross_project_master_messages_use_shared_host_discovery_and_replay_target_mailbox() {
    let project_a = temp_root("cross-project-a");
    let project_b = temp_root("cross-project-b");
    let host = temp_root("cross-project-host");

    register_scope_with_host(&project_a, &host, "scope-a", "app-a", &["master-a"]);
    register_scope_with_host(&project_b, &host, "scope-b", "app-b", &["master-b"]);
    register_agent_with_host(
        &project_a, &host, "scope-a", "master-a", "master-a", "master", None,
    );
    register_agent_with_host(
        &project_b, &host, "scope-b", "master-b", "master-b", "master", None,
    );

    let a_to_b = call_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-b", "sessionId": "master-b" },
                "title": "cross project request",
                "priority": "p1",
                "body": "A asks B to coordinate"
            }
        }),
    );
    assert_eq!(a_to_b["route"]["mode"], "cross-scope-master");
    assert_eq!(a_to_b["route"]["sameAppserver"], false);
    assert_eq!(a_to_b["route"]["sameProject"], false);
    assert_eq!(a_to_b["message"]["state"], "accepted");

    let b_to_a = call_with_host(
        &project_b,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-b", "sessionId": "master-b" },
                "to": { "scopeId": "scope-a", "sessionId": "master-a" },
                "title": "cross project response",
                "priority": "p1",
                "body": "B confirms coordination"
            }
        }),
    );
    assert_eq!(b_to_a["route"]["mode"], "cross-scope-master");

    let status_a = call_with_host(&project_a, &host, json!({ "op": "status" }));
    assert_eq!(status_a["messages"].as_array().unwrap().len(), 1);
    assert_eq!(status_a["messages"][0]["to"]["sessionId"], "master-b");
    let status_b = call_with_host(&project_b, &host, json!({ "op": "status" }));
    assert_eq!(status_b["messages"].as_array().unwrap().len(), 1);
    assert_eq!(status_b["messages"][0]["to"]["sessionId"], "master-a");

    let registry = fs::read_to_string(host.join("communication.jsonl")).unwrap();
    assert_eq!(registry.lines().count(), 4);
    assert!(registry.contains("\"scopeId\":\"scope-a\""));
    assert!(registry.contains("\"scopeId\":\"scope-b\""));

    fs::remove_dir_all(project_a).unwrap();
    fs::remove_dir_all(project_b).unwrap();
    fs::remove_dir_all(host).unwrap();
}

#[test]
fn cross_project_peer_and_unknown_target_remain_fail_closed_after_discovery() {
    let project_a = temp_root("cross-project-peer-a");
    let project_b = temp_root("cross-project-peer-b");
    let host = temp_root("cross-project-peer-host");

    register_scope_with_host(
        &project_a,
        &host,
        "scope-a",
        "app-a",
        &["master-a", "peer-a"],
    );
    register_scope_with_host(
        &project_b,
        &host,
        "scope-b",
        "app-b",
        &["master-b", "peer-b"],
    );
    register_agent_with_host(
        &project_a, &host, "scope-a", "master-a", "master-a", "master", None,
    );
    register_agent_with_host(
        &project_a, &host, "scope-a", "peer-a", "peer-a", "peer", None,
    );
    register_agent_with_host(
        &project_b, &host, "scope-b", "master-b", "master-b", "master", None,
    );
    register_agent_with_host(
        &project_b, &host, "scope-b", "peer-b", "peer-b", "peer", None,
    );

    let peer_cross = call_error_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-a" },
                "to": { "scopeId": "scope-b", "sessionId": "master-b" },
                "title": "forbidden cross project peer",
                "priority": "p2",
                "body": "peer cannot cross scope"
            }
        }),
    );
    assert!(
        peer_cross.contains("cross_scope_master_required"),
        "{peer_cross}"
    );

    let unknown = call_error_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-z", "sessionId": "master-z" },
                "title": "unknown target",
                "priority": "p2",
                "body": "must fail closed"
            }
        }),
    );
    assert!(unknown.contains("agent_not_registered"), "{unknown}");

    let status_a = call_with_host(&project_a, &host, json!({ "op": "status" }));
    assert!(status_a["messages"].as_array().unwrap().is_empty());
    fs::remove_dir_all(project_a).unwrap();
    fs::remove_dir_all(project_b).unwrap();
    fs::remove_dir_all(host).unwrap();
}

#[test]
fn missing_host_discovery_index_is_unavailable_not_unknown_agent() {
    let project_a = temp_root("missing-host-index-a");
    let host = temp_root("missing-host-index-host");

    register_scope_with_host(&project_a, &host, "scope-a", "app-a", &["master-a"]);
    register_agent_with_host(
        &project_a, &host, "scope-a", "master-a", "master-a", "master", None,
    );

    fs::remove_file(host.join("communication.jsonl")).unwrap();

    let missing = call_error_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-z", "sessionId": "master-z" },
                "title": "target registry missing",
                "priority": "p2",
                "body": "must not be classified as unknown agent"
            }
        }),
    );
    assert!(
        missing.contains("GLOBAL_COMMUNICATION_REGISTRY_UNAVAILABLE"),
        "{missing}"
    );
    assert!(!missing.contains("agent_not_registered"), "{missing}");

    register_scope_with_host(&project_a, &host, "scope-a", "app-a", &["master-a"]);
    register_agent_with_host(
        &project_a, &host, "scope-a", "master-a", "master-a", "master", None,
    );

    let unknown = call_error_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-z", "sessionId": "master-z" },
                "title": "unknown target",
                "priority": "p2",
                "body": "valid unknown agent must stay fail closed"
            }
        }),
    );
    assert!(unknown.contains("agent_not_registered"), "{unknown}");
    assert!(
        !unknown.contains("GLOBAL_COMMUNICATION_REGISTRY_UNAVAILABLE"),
        "{unknown}"
    );

    fs::remove_dir_all(project_a).unwrap();
    fs::remove_dir_all(host).unwrap();
}

#[test]
fn discovery_registration_failure_replays_from_local_pending_intent() {
    let root = temp_root("discovery-registration-recovery");
    let host = root.join(".appsdk-host");
    let project_root = root.canonicalize().unwrap();
    let project_root = project_root.to_str().unwrap();
    call_with_host(
        &root,
        &host,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "runtime-recovery",
                "appserverId": "app-recovery",
                "namespace": "codex_tui",
                "endpoint": "mock://recovery",
                "projectRoot": project_root,
                "capabilities": ["send_message_to_thread"],
                "processId": std::process::id()
            }
        }),
    );
    let registry_file = host.join("communication.jsonl");
    fs::create_dir_all(&registry_file).unwrap();
    let error = call_error_with_host(
        &root,
        &host,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": "scope-recovery",
                "appserverId": "app-recovery",
                "namespace": "codex_tui",
                "endpoint": "mock://recovery",
                "projectRoot": project_root,
                "sessionIds": ["master"],
                "runtimeId": "runtime-recovery"
            }
        }),
    );
    assert!(
        error.contains("communication_discovery_registration_failed"),
        "{error}"
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let raw = fs::read_to_string(&mailbox).unwrap();
    assert!(raw.contains("\"kind\":\"discovery.pending\""));
    assert!(raw.contains("\"kind\":\"scope.registered\""));

    fs::remove_dir_all(&registry_file).unwrap();
    let status = call_with_host(&root, &host, json!({ "op": "status" }));
    assert_eq!(status["scopes"].as_array().unwrap().len(), 1);
    assert_eq!(status["discoveryPending"].as_array().unwrap().len(), 0);
    assert!(host.join("communication.jsonl").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rebind_failure_replays_from_local_pending_intent() {
    let root = temp_root("discovery-rebind-recovery");
    let host = root.join(".appsdk-host");
    register_scope_with_host(
        &root,
        &host,
        "scope-rebind-recovery",
        "app-rebind-recovery",
        &["master-old", "master-new"],
    );
    register_agent_with_host(
        &root,
        &host,
        "scope-rebind-recovery",
        "master-old",
        "master",
        "master",
        None,
    );

    let registry_file = host.join("communication.jsonl");
    let registry_backup = host.join("communication.jsonl.backup");
    fs::rename(&registry_file, &registry_backup).unwrap();
    fs::create_dir_all(&registry_file).unwrap();
    let error = call_error_with_host(
        &root,
        &host,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope-rebind-recovery", "sessionId": "master-old" },
                "to": { "scopeId": "scope-rebind-recovery", "sessionId": "master-new" },
                "runtimeId": "runtime-scope-rebind-recovery"
            }
        }),
    );
    assert!(
        error.contains("communication_discovery_registration_failed"),
        "{error}"
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let raw = fs::read_to_string(&mailbox).unwrap();
    assert!(raw.contains("\"kind\":\"discovery.pending\""));
    assert!(raw.contains("\"kind\":\"agent.rebound\""));

    fs::remove_dir_all(&registry_file).unwrap();
    fs::rename(&registry_backup, &registry_file).unwrap();
    let status = call_with_host(&root, &host, json!({ "op": "status" }));
    assert_eq!(status["agents"].as_array().unwrap().len(), 1);
    assert_eq!(status["agents"][0]["sessionId"], "master-new");
    assert_eq!(status["discoveryPending"].as_array().unwrap().len(), 0);
    let registry = fs::read_to_string(&registry_file).unwrap();
    assert!(registry.contains("\"event\":\"communication.agent.rebound\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rebind_recovery_rejects_projection_key_collision_before_journal_commit() {
    let root = temp_root("discovery-rebind-recovery-collision");
    let host = root.join(".appsdk-host");
    register_scope_with_host(
        &root,
        &host,
        "scope-rebind-recovery-collision",
        "app-rebind-recovery-collision",
        &["master-old", "master-new"],
    );
    register_agent_with_host(
        &root,
        &host,
        "scope-rebind-recovery-collision",
        "master-old",
        "master",
        "master",
        None,
    );
    call_with_host(
        &root,
        &host,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope-rebind-recovery-collision", "sessionId": "master-old" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );

    let registry_file = host.join("communication.jsonl");
    let registry_backup = host.join("communication.jsonl.backup");
    fs::rename(&registry_file, &registry_backup).unwrap();
    fs::create_dir_all(&registry_file).unwrap();
    let error = call_error_with_host(
        &root,
        &host,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope-rebind-recovery-collision", "sessionId": "master-old" },
                "to": { "scopeId": "scope-rebind-recovery-collision", "sessionId": "master-new" },
                "runtimeId": "runtime-scope-rebind-recovery-collision"
            }
        }),
    );
    assert!(
        error.contains("communication_discovery_registration_failed"),
        "{error}"
    );

    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut lines: Vec<Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let pending_index = lines
        .iter()
        .rposition(|event| {
            event["kind"] == "discovery.pending"
                && event["data"]["operation"]["operation"] == "rebind"
        })
        .unwrap();
    lines.truncate(pending_index + 1);
    let orphan = json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "event-orphan-recovery-collision",
        "at": "2026-01-01T00:00:00Z",
        "kind": "wakeup.updated",
        "data": {
            "address": { "scopeId": "scope-rebind-recovery-collision", "sessionId": "master-new" },
            "idleSince": null,
            "remindersSent": 0,
            "nextDueAt": null,
            "stopped": false,
            "lastReminderAt": null
        }
    });
    lines.push(orphan.clone());
    let rewritten = lines
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&mailbox, format!("{rewritten}\n")).unwrap();

    fs::remove_dir_all(&registry_file).unwrap();
    fs::rename(&registry_backup, &registry_file).unwrap();
    let error = call_error_with_host(&root, &host, json!({ "op": "status" }));
    assert!(
        error.contains("communication_discovery_recovery_failed")
            && error.contains("duplicate wakeup record"),
        "{error}"
    );
    let after = fs::read_to_string(&mailbox).unwrap();
    assert_eq!(after.matches("\"kind\":\"agent.rebound\"").count(), 0);
    assert!(after.contains("\"kind\":\"discovery.pending\""));
    fs::remove_dir_all(root).unwrap();
}
