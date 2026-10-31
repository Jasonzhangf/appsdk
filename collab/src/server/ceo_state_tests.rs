//! Persistence tests for host-owned CEO coordination facts in the shared reducer.
//!
//! These exercise the reducer contract only: the real journal owner still
//! commits the same typed events, and replay uses `snapshot_events`.

use crate::ceo::{CeoBootstrapRecord, CeoGrant};
use crate::proto::{ReplyPolicy, Resp};
use crate::server::state::{
    responsibility_preflight, Event, ReplyObligation, ResponsibilityFence, ResponsibilitySnapshot,
    State,
};
use serde_json::json;

fn replay(state: &State) -> State {
    state
        .snapshot_events()
        .into_iter()
        .fold(State::default(), |mut replayed, event| {
            replayed.apply(&event);
            replayed
        })
}

fn grant(workspace: &str, granted_ms: i64) -> CeoGrant {
    CeoGrant {
        workspace: workspace.into(),
        app_scope_id: "appsdk".into(),
        name: "ceo".into(),
        granted_by: "user".into(),
        approval: "approved".into(),
        granted_ms,
    }
}

fn bootstrap(operation_id: &str, phase: &str) -> CeoBootstrapRecord {
    CeoBootstrapRecord {
        operation_id: operation_id.into(),
        project: "/tmp/target-project".into(),
        approval: "approved".into(),
        profile: "gcm".into(),
        tmux_socket: "/tmp/target.sock".into(),
        tmux_session: "target".into(),
        requested_by: "ceo".into(),
        requested_workspace: "/workspace".into(),
        requested_app_scope: "appserver-cli".into(),
        phase: phase.into(),
        pane_id: None,
        error: None,
        handshake_message_id: None,
        cancellation_approval: None,
        updated_ms: 1,
    }
}

fn cross_project_obligation() -> ReplyObligation {
    ReplyObligation {
        message_id: "m-1".into(),
        project: "/tmp/target-project".into(),
        from_project: Some("/tmp/origin-project".into()),
        from_ceo: true,
        from_worker_id: "ceo".into(),
        from_name: Some("ceo".into()),
        recipient_worker_id: "master".into(),
        recipient_name: Some("master".into()),
        policy: ReplyPolicy::Required,
        created_ms: 30,
        reminded_idle_episode: false,
    }
}

#[test]
fn ceo_grant_set_replace_clear_survives_snapshot_replay() {
    let mut state = State::default();
    state.apply(&Event::CeoGrantSet {
        grant: Some(grant("/tmp/ws-a", 10)),
    });
    state.apply(&Event::CeoBootstrapSet {
        record: bootstrap("op-1", "prepared"),
    });

    let replayed = replay(&state);
    assert_eq!(
        replayed
            .ceo_grant
            .as_ref()
            .map(|grant| grant.workspace.as_str()),
        Some("/tmp/ws-a")
    );
    assert_eq!(replayed.ceo_bootstraps.len(), 1);

    let mut replaced = replayed;
    replaced.apply(&Event::CeoGrantSet {
        grant: Some(grant("/tmp/ws-b", 20)),
    });
    let replayed = replay(&replaced);
    assert_eq!(
        replayed
            .ceo_grant
            .as_ref()
            .map(|grant| grant.workspace.as_str()),
        Some("/tmp/ws-b")
    );

    let mut cleared = replayed;
    cleared.apply(&Event::CeoGrantSet { grant: None });
    assert!(cleared.ceo_grant.is_none());
    let replayed = replay(&cleared);
    assert!(replayed.ceo_grant.is_none());
    assert_eq!(
        replayed.ceo_bootstraps.len(),
        1,
        "clearing the grant must retain bootstrap history"
    );
}

#[test]
fn ceo_bootstrap_phase_and_cross_project_reply_survive_snapshot_replay() {
    let mut state = State::default();
    state.apply(&Event::CeoBootstrapSet {
        record: bootstrap("op-1", "prepared"),
    });
    state.apply(&Event::CeoBootstrapSet {
        record: CeoBootstrapRecord {
            phase: "launched".into(),
            pane_id: Some("%7".into()),
            updated_ms: 20,
            ..bootstrap("op-1", "launched")
        },
    });
    state.apply(&Event::ReplyObligationSet {
        obligation: cross_project_obligation(),
    });

    let replayed = replay(&state);
    let record = replayed.ceo_bootstraps.get("op-1").unwrap();
    assert_eq!(record.phase, "launched");
    assert_eq!(record.pane_id.as_deref(), Some("%7"));
    assert_eq!(record.updated_ms, 20);
    let obligation = replayed.reply_obligations.get("m-1").unwrap();
    assert_eq!(
        obligation.from_project.as_deref(),
        Some("/tmp/origin-project")
    );
}

#[test]
fn legacy_reply_obligation_without_from_project_decodes() {
    let legacy = serde_json::json!({
        "message_id": "m-1",
        "project": "/tmp/target-project",
        "from_worker_id": "ceo",
        "recipient_worker_id": "master",
        "policy": "required",
        "created_ms": 30
    });
    let obligation: ReplyObligation = serde_json::from_value(legacy).unwrap();
    assert!(obligation.from_project.is_none());

    let wire = serde_json::to_value(&obligation).unwrap();
    assert!(wire.get("from_project").is_none());

    let mut state = State::default();
    state.apply(&Event::ReplyObligationSet { obligation });
    assert!(state.reply_obligations["m-1"].from_project.is_none());
}

#[test]
fn ceo_events_do_not_change_business_task_responsibility() {
    let mut state = State::default();
    state.apply(&Event::ResponsibilityFenceSet {
        fence: ResponsibilityFence {
            operation_id: "op-close".into(),
            worker_id: "closing-peer".into(),
            project_scope: "/tmp/target-project".into(),
            app_scope: "tui-default".into(),
            binding_id: "binding-closing-peer".into(),
            endpoint_generation: 1,
            snapshot: ResponsibilitySnapshot::default(),
            state: "closing".into(),
            created_ms: 1,
        },
    });

    let events = [
        Event::CeoGrantSet {
            grant: Some(grant("/tmp/ws", 10)),
        },
        Event::CeoBootstrapSet {
            record: bootstrap("op-1", "prepared"),
        },
    ];
    assert_eq!(responsibility_preflight(&state, &events), Ok(()));
}

/// Handler-level fixture: a real reducer-backed `Server` plus a manager, no
/// Unix listener. A disabled quiet window (`start == end`) makes the ordinary
/// hourly policy deterministic so the urgent-bypass path can be exercised after
/// one acknowledgement establishes the cooldown.
fn ceo_handler_fixture() -> (
    std::sync::Arc<crate::server::Server>,
    std::sync::Arc<super::ProjectRuntimeManager>,
) {
    let (mut server, _root) = super::peer_tests::test_server();
    server.config.ceo_report.quiet_window_start_minute = 0;
    server.config.ceo_report.quiet_window_end_minute = 0;
    let server = std::sync::Arc::new(server);
    let manager = super::ProjectRuntimeManager::new(server.clone(), &server.host_paths)
        .expect("build manager without a listener");
    (server, manager)
}

#[test]
fn ceo_handler_progress_records_trims_and_dedups_exact_content() {
    let (server, manager) = ceo_handler_fixture();
    let project = server.root.to_string_lossy().into_owned();

    let first = manager
        .ceo_record_progress(
            &server,
            "progress subject".into(),
            "  progress body  ".into(),
            false,
        )
        .expect("first progress records");
    assert_eq!(first["recorded"], true, "{first}");
    assert_eq!(first["deduplicated"], false, "{first}");
    assert_eq!(first["revision"], 1, "{first}");
    assert_eq!(first["project"], project, "{first}");

    // Edge whitespace is trimmed, so the identical business content is a dedup.
    let repeat = manager
        .ceo_record_progress(
            &server,
            "progress subject".into(),
            "progress body".into(),
            false,
        )
        .expect("repeat progress is accepted as a no-op");
    assert_eq!(repeat["recorded"], false, "{repeat}");
    assert_eq!(repeat["deduplicated"], true, "{repeat}");
    assert_eq!(repeat["revision"], 1, "{repeat}");

    // A typed urgent signal for equal content is a new meaningful revision.
    let urgent = manager
        .ceo_record_progress(
            &server,
            "progress subject".into(),
            "progress body".into(),
            true,
        )
        .expect("typed urgent upgrade records");
    assert_eq!(urgent["recorded"], true, "{urgent}");
    assert_eq!(urgent["revision"], 2, "{urgent}");
    assert_eq!(urgent["urgent"], true, "{urgent}");

    // Empty content is rejected on the wire even if the CLI trim is bypassed.
    assert!(manager
        .ceo_record_progress(&server, "subject".into(), "   ".into(), false)
        .is_err());
}

#[test]
fn ceo_handler_batch_ack_is_urgent_only_during_ordinary_suppression() {
    let (server, manager) = ceo_handler_fixture();
    let (other_server, _other_root) = super::peer_tests::test_server();
    let other_server = std::sync::Arc::new(other_server);

    // Ordinary progress is allowed before any report, so a normal digest batch
    // covers it.
    manager
        .ceo_record_progress(&server, "alpha".into(), "first delivery".into(), false)
        .expect("record ordinary progress");
    let ordinary = manager
        .ceo_prepare_report_batch(None)
        .expect("prepare ordinary batch");
    assert_eq!(ordinary["suppressed"], false, "{ordinary}");
    assert_eq!(
        ordinary["coverage"].as_object().unwrap().len(),
        1,
        "{ordinary}"
    );
    let ordinary_id = ordinary["batch_id"]
        .as_str()
        .expect("ordinary batch id")
        .to_owned();

    // Acknowledging after visible output starts the hourly ordinary cooldown.
    let ack = manager
        .ceo_report_ack(ordinary_id.clone())
        .expect("ack ordinary batch");
    assert_eq!(ack.data["batch_id"], ordinary_id, "{:?}", ack.data);
    assert_eq!(ack.data["pending_revision_count"], 0, "{:?}", ack.data);

    // During cooldown an ordinary update and an urgent update arrive. Only the
    // explicitly typed urgent project may pass.
    manager
        .ceo_record_progress(&server, "alpha".into(), "ordinary later".into(), false)
        .expect("record newer ordinary progress");
    manager
        .ceo_record_progress(&other_server, "beta".into(), "urgent now".into(), true)
        .expect("record urgent progress");

    let suppressed = manager
        .ceo_prepare_report_batch(None)
        .expect("prepare suppressed batch");
    assert_eq!(suppressed["suppressed"], true, "{suppressed}");
    assert_eq!(suppressed["excluded_project_count"], 1, "{suppressed}");
    let beta_key = other_server.root.to_string_lossy().into_owned();
    let coverage = suppressed["coverage"].as_object().unwrap();
    assert_eq!(
        coverage.len(),
        1,
        "ordinary project leaked into urgent batch: {suppressed}"
    );
    assert!(coverage.contains_key(&beta_key), "{suppressed}");
    let urgent_id = suppressed["batch_id"]
        .as_str()
        .expect("urgent batch id")
        .to_owned();

    // Repeating the read returns the exact prepared batch, not a new identity.
    let repeated = manager
        .ceo_prepare_report_batch(None)
        .expect("repeat suppressed read");
    assert_eq!(repeated["batch_id"], urgent_id, "{repeated}");

    // Unknown and repeated acknowledgements are rejected without mutation.
    assert!(manager.ceo_report_ack("missing-batch".into()).is_err());
    manager
        .ceo_report_ack(urgent_id.clone())
        .expect("ack urgent batch");
    assert!(manager.ceo_report_ack(urgent_id).is_err());

    // The ordinary alpha revision stays pending after the urgent batch is acked.
    let after = manager
        .ceo_prepare_report_batch(None)
        .expect("read after urgent ack");
    assert!(after["batch_id"].is_null(), "{after}");
    assert_eq!(after["suppressed"], true, "{after}");
    assert_eq!(after["excluded_project_count"], 1, "{after}");
}

#[test]
fn ceo_handler_durable_reply_keeps_receipt_when_progress_record_fails() {
    let (server, manager) = ceo_handler_fixture();
    server.state.lock().unwrap().journal_poison = Some("synthetic journal failure".into());

    let delivered = Ok(Resp::data(json!({"msg_id":"m-reply","durable":true})));
    let response = manager
        .attach_solicited_progress(
            delivered,
            &server,
            "progress subject".into(),
            "progress body".into(),
            false,
        )
        .expect("a durable reply must keep its receipt when progress cannot be recorded");

    assert_eq!(response.data["msg_id"], "m-reply", "{}", response.data);
    assert_eq!(response.data["durable"], true, "{}", response.data);
    assert_eq!(
        response.data["progress_recorded"], false,
        "{}",
        response.data
    );
    assert!(!response.ok, "partial failure must stay visible");
    assert!(
        response
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("do not resend"),
        "partial failure must forbid an uncertain resend: {:?}",
        response.error
    );
}

#[test]
fn ceo_handler_durable_marker_failure_records_progress_without_clearing_receipt() {
    let (server, manager) = ceo_handler_fixture();
    let delivered = Err(Resp::err_data(
        "CEO_REPLY_MARKER_COMMIT_FAILED: reply is durable; do not resend",
        json!({"msg_id":"m-durable","durable":true,"reply_marker_cleared":false}),
    ));

    let response = manager
        .attach_solicited_progress(
            delivered,
            &server,
            "progress subject".into(),
            "progress body".into(),
            false,
        )
        .expect_err("marker failure is still a partial failure");

    assert_eq!(response.data["msg_id"], "m-durable", "{}", response.data);
    assert_eq!(response.data["recorded"], true, "{}", response.data);
    assert_eq!(response.data["revision"], 1, "{}", response.data);
    assert!(
        server.state.lock().unwrap().reply_obligations.is_empty(),
        "recording progress alone must never create or clear a reply obligation"
    );
}

#[test]
fn ceo_handler_unsolicited_progress_claims_no_notification_or_report() {
    let (server, manager) = ceo_handler_fixture();
    let first = manager
        .ceo_record_unsolicited_progress(&server, "subject".into(), "body".into(), true)
        .expect("first unsolicited progress records");
    assert_eq!(first.data["recorded"], true, "{:?}", first.data);
    assert_eq!(first.data["urgent"], true, "{:?}", first.data);
    assert_eq!(first.data["notified"], false, "{:?}", first.data);
    assert_eq!(first.data["reported"], false, "{:?}", first.data);

    let repeat = manager
        .ceo_record_unsolicited_progress(&server, "subject".into(), "body".into(), true)
        .expect("duplicate unsolicited progress is a no-op");
    assert_eq!(repeat.data["recorded"], false, "{:?}", repeat.data);
    assert_eq!(repeat.data["deduplicated"], true, "{:?}", repeat.data);
    assert_eq!(repeat.data["revision"], 1, "{:?}", repeat.data);
}

#[test]
fn ceo_handler_prepare_batch_reports_the_stored_batch_id() {
    let (server, manager) = ceo_handler_fixture();
    manager
        .ceo_record_progress(&server, "alpha".into(), "first delivery".into(), false)
        .expect("record ordinary progress");

    let first = manager
        .ceo_prepare_report_batch(None)
        .expect("prepare batch");
    let batch_id = first["batch_id"]
        .as_str()
        .expect("prepared batch id")
        .to_owned();
    {
        let state = server.state.lock().unwrap();
        let stored = state
            .ceo_report_state
            .batches
            .get(&batch_id)
            .expect("response must name the persisted batch");
        assert_eq!(stored.coverage.len(), 1, "{stored:?}");
    }

    let repeat = manager
        .ceo_prepare_report_batch(None)
        .expect("repeat prepare");
    assert_eq!(repeat["batch_id"], batch_id, "{repeat}");
}

#[test]
fn ceo_promote_guidance_preserves_user_content_and_reports_partial_failure() {
    let (server, manager) = ceo_handler_fixture();
    let workspace = server.root.join("ceo-workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    std::fs::write(workspace.join("AGENTS.md"), "# User rules\n\nkeep me\n")
        .expect("seed user content");

    let grant = CeoGrant {
        workspace: workspace.to_string_lossy().into_owned(),
        app_scope_id: "appsdk".into(),
        name: "ceo".into(),
        granted_by: "user".into(),
        approval: "approved".into(),
        granted_ms: 1,
    };
    let mut status = Resp::data(json!({"is_current": true}));
    manager.attach_ceo_workspace_guidance(&mut status, &grant);
    assert!(status.ok, "guidance failed: {:?}", status.error);
    assert_eq!(
        status.data["workspace_guidance"]["action"], "changed",
        "{}",
        status.data
    );
    let text = std::fs::read_to_string(workspace.join("AGENTS.md")).expect("read guidance");
    assert!(
        text.starts_with("# User rules\n\nkeep me\n"),
        "user content was rewritten: {text}"
    );
    assert!(
        text.contains(crate::ceo_workspace_guidance::MANAGED_BLOCK_START),
        "{text}"
    );
    assert!(text.contains("USER.md"), "{text}");
    assert!(text.contains("skills/collab/SKILL.md"), "{text}");

    // Repeating the same promote guidance is idempotent and rewrites nothing.
    let mut repeat = Resp::data(json!({"is_current": true}));
    manager.attach_ceo_workspace_guidance(&mut repeat, &grant);
    assert_eq!(
        repeat.data["workspace_guidance"]["action"], "noop",
        "{}",
        repeat.data
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("AGENTS.md")).expect("reread guidance"),
        text,
        "repeated guidance rewrote user content"
    );

    let missing = CeoGrant {
        workspace: workspace.join("missing").to_string_lossy().into_owned(),
        ..grant
    };
    let mut status = Resp::data(json!({"is_current": true}));
    manager.attach_ceo_workspace_guidance(&mut status, &missing);
    assert!(!status.ok, "missing workspace must stay visible");
    assert_eq!(
        status.data["workspace_guidance"]["grant_durable"], true,
        "{}",
        status.data
    );
    assert!(
        status
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("CEO_GRANT_RECORDED_BUT_WORKSPACE_GUIDANCE_FAILED"),
        "{:?}",
        status.error
    );
}

#[test]
fn ceo_progress_projects_expose_canonical_master_workspace_idle() {
    let (server, root) = super::peer_tests::test_server();
    let transport = super::peer_tests::test_appserver_transport("thread-master");
    let app = crate::identity::AppServerId::new("appserver-cli").unwrap();
    let cwd = server.root.display().to_string();
    let registered = super::peer_tests::seed_committed_owner(
        &server,
        "master",
        "token-master",
        &transport,
        &cwd,
        Some(app.clone()),
    );
    assert!(registered.ok, "master registration failed: {registered:?}");
    super::commit_current_thread_route_for_runtime(&server, &server, "master", &cwd, Some(&app))
        .expect("master native thread route must commit");
    super::restore_registered_peer_default_leases(&server);
    let promoted = super::handle_master_promote(
        &server,
        "master".into(),
        "token-master".into(),
        "user-approved".into(),
    );
    assert!(promoted.ok, "master promotion failed: {promoted:?}");

    let scope =
        super::GlobalState::canonical_project_scope(&server.root).expect("canonical project scope");
    let session = server
        .state
        .lock()
        .unwrap()
        .workers
        .get("master")
        .and_then(|worker| worker.transport.as_ref())
        .and_then(|transport| transport.session_id.clone())
        .expect("master transport session");
    let now = super::now_ms();
    server.commit(&[
        Event::AgentPresenceObserved {
            presence: super::name_state::AgentPresenceRecord {
                project_scope: scope.as_str().into(),
                worker_id: "master".into(),
                session_id: session.clone(),
                presence: super::name_state::AgentPresence::Online,
                reason: "ceo progress fixture".into(),
                observed_ms: now,
            },
        },
        Event::AgentActivityObserved {
            activity: super::name_state::AgentActivityRecord {
                project_scope: scope.as_str().into(),
                worker_id: "master".into(),
                session_id: session,
                activity: super::name_state::AgentActivity::Idle,
                background: false,
                reason: "ceo progress fixture".into(),
                observed_ms: now,
                idle_since_ms: Some(now - 60_000),
            },
        },
    ]);

    let server = std::sync::Arc::new(server);
    let manager = super::ProjectRuntimeManager::new(server.clone(), &server.host_paths)
        .expect("build manager without a listener");
    let response = manager
        .ceo_progress(Some(root.display().to_string()))
        .expect("ceo progress reads the registered project");
    assert!(response.ok, "ceo progress failed: {response:?}");
    let project = response.data["projects"]
        .as_array()
        .and_then(|projects| {
            projects
                .iter()
                .find(|project| project["project"].as_str() == Some(scope.as_str()))
        })
        .expect("progress includes the canonical project");
    let workspace = &project["workspace"];
    assert_eq!(workspace["project_root"], scope.as_str());
    assert_eq!(workspace["activity"], "idle");
    assert_eq!(workspace["idle_since_ms"], now - 60_000);
    assert!(
        workspace["idle_duration_ms"].as_i64().unwrap() >= 60_000,
        "idle duration derives from the trusted start: {workspace}"
    );
    std::fs::remove_dir_all(root).ok();
}

/// A CEO must be able to read, per project, whether the subordinate master has
/// its project patrol armed. The fact reuses the same armed `master-idle`
/// subscription the master reads through `collab context`; the negative case
/// keeps a project without patrol explicit instead of guessing.
#[test]
fn ceo_progress_reports_master_patrol_armed_state() {
    let (server, root) = super::peer_tests::test_server();
    super::peer_tests::register_synthetic_peer(&server, "master", "%master");
    let promoted = super::handle_master_promote(
        &server,
        "master".into(),
        "token-master".into(),
        "user-approved".into(),
    );
    assert!(promoted.ok, "master promotion failed: {promoted:?}");

    let scope =
        super::GlobalState::canonical_project_scope(&server.root).expect("canonical project scope");
    let server = std::sync::Arc::new(server);
    let manager = super::ProjectRuntimeManager::new(server.clone(), &server.host_paths)
        .expect("build manager without a listener");
    let progress = |manager: &super::ProjectRuntimeManager, root: &std::path::Path| {
        manager
            .ceo_progress(Some(root.display().to_string()))
            .expect("ceo progress reads the registered project")
    };
    let unarmed = progress(&manager, &root);
    assert!(unarmed.ok, "ceo progress failed: {unarmed:?}");
    let project = unarmed.data["projects"]
        .as_array()
        .and_then(|projects| {
            projects
                .iter()
                .find(|project| project["project"].as_str() == Some(scope.as_str()))
        })
        .expect("progress includes the canonical project");
    assert_eq!(
        project["patrol"]["armed"],
        json!(false),
        "an unarmed master patrol must read as not armed: {project}"
    );

    let now = super::now_ms();
    server.commit(&[Event::NotificationSubscribed {
        subscription: super::state::NotificationSubscription {
            id: "sub-master-idle".into(),
            worker_id: "master".into(),
            event: "master-idle".into(),
            subject: Some("master-idle".into()),
            target: "thread-master".into(),
            method: "tmux".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: Some(900_000),
            repeat_count: 1,
            fired_count: 0,
            expires_ms: now + 3_600_000,
            status: "armed".into(),
            created_ms: now,
            updated_ms: now,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
        },
    }]);
    let armed = progress(&manager, &root);
    assert!(armed.ok, "ceo progress failed: {armed:?}");
    let project = armed.data["projects"]
        .as_array()
        .and_then(|projects| {
            projects
                .iter()
                .find(|project| project["project"].as_str() == Some(scope.as_str()))
        })
        .expect("progress includes the canonical project");
    assert_eq!(
        project["patrol"]["armed"],
        json!(true),
        "an armed master patrol must be visible to the CEO: {project}"
    );
    assert_eq!(project["patrol"]["interval_ms"], json!(900_000), "{project}");
    std::fs::remove_dir_all(root).ok();
}
