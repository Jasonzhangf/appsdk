//! Host control commits. No project reducer, mailbox, worker or permission state.
use super::global_state::CommandReceipt;
use super::notification_contract::{CommandOutcome, CommitReceipt, MutationError};
use crate::ceo::{CeoBootstrapRecord, CeoGrant};
use crate::ceo_report_state::{CeoNoticeGate, CeoProgressContent, CeoReportState};
use crate::identity::{CommandId, OperationId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

pub(super) enum HostChange {
    CeoGrantSet {
        grant: Option<CeoGrant>,
    },
    CeoBootstrapSet {
        record: CeoBootstrapRecord,
    },
    CeoProgressRecorded {
        project: String,
        content: CeoProgressContent,
        urgent: bool,
    },
    CeoReportBatchPrepared {
        batch_id: String,
        coverage: BTreeMap<String, u64>,
        urgent: bool,
    },
    CeoReportBatchAcknowledged {
        batch_id: String,
        reported_ms: u64,
    },
    CeoReportNoticeGateUpdated {
        gate: CeoNoticeGate,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_ceo_resolution_uses_current_host_control() {
        use super::super::*;
        let (mut server, root) = peer_tests::test_server();
        let canonical = root.canonicalize().unwrap();
        server.root = canonical.clone();
        server.storage_root = canonical.clone();
        let server = Arc::new(server);
        peer_tests::register_appserver_worker(&server, &canonical, "host-ceo", "host-ceo-thread");
        let route = server_route_scope(&server, &server.state.lock().unwrap())
            .unwrap()
            .unwrap();
        let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
            .load()
            .unwrap();
        let name = control
            .key_for_worker(&route.project_scope_id, &route.app_scope_id, "host-ceo")
            .unwrap()
            .unwrap()
            .name
            .clone();
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        owner
            .commit_checked(&[HostChange::CeoGrantSet {
                grant: Some(CeoGrant {
                    workspace: canonical.to_string_lossy().into_owned(),
                    app_scope_id: route.app_scope_id.as_str().into(),
                    name,
                    granted_by: "user".into(),
                    approval: "fixture".into(),
                    granted_ms: 1,
                }),
            }])
            .unwrap();
        let state = server.state.lock().unwrap();
        assert!(state.ceo_grant.is_none());
        let resolved = resolve_send_name_address(&server, &state, &control, "ceo".into()).unwrap();
        assert_eq!(resolved.0, "host-ceo");
        assert!(is_current_ceo_holder(&server, &state, "host-ceo").unwrap());
        let inbound = resolve_channel_inbound_target(
            &server,
            &state,
            "qqbot",
            "fixture",
            "conversation",
            "[ceo] host body",
            "",
        )
        .unwrap();
        assert_eq!(inbound.to, "host-ceo");
        assert_eq!(inbound.body, "host body");
        drop(state);
        owner
            .commit_checked(&[HostChange::CeoGrantSet { grant: None }])
            .unwrap();
        let mut state = server.state.lock().unwrap();
        state.ceo_grant = Some(CeoGrant {
            workspace: canonical.to_string_lossy().into_owned(),
            app_scope_id: route.app_scope_id.as_str().into(),
            name: "Alice".into(),
            granted_by: "stale-project".into(),
            approval: "obsolete".into(),
            granted_ms: 1,
        });
        let error = resolve_send_name_address(&server, &state, &control, "ceo".into()).unwrap_err();
        assert!(error.contains("CHANNEL_CEO_GRANT_MISSING"), "{error}");
        assert!(!is_current_ceo_holder(&server, &state, "host-ceo").unwrap());
        let error = resolve_channel_inbound_target(
            &server,
            &state,
            "qqbot",
            "fixture",
            "conversation",
            "[ceo] denied",
            "",
        )
        .err()
        .unwrap();
        assert!(error.contains("CHANNEL_CEO_GRANT_MISSING"), "{error}");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn dispatch_attaches_only_the_selected_project() {
        use super::super::*;
        let (server, root) = peer_tests::test_server();
        let host = Arc::new(server);
        let manager = ProjectRuntimeManager::new(host.clone(), &host.host_paths).unwrap();
        let context = ProjectContext {
            project_scope: GlobalState::canonical_project_scope(&root).unwrap(),
            app_scope_id: crate::identity::AppServerId::new(crate::identity::CLI_APP_SERVER_ID)
                .unwrap(),
            canonical_root: root.canonicalize().unwrap().to_string_lossy().into_owned(),
            runtime_context: None,
        };
        let before = std::fs::read(&host.state_path).unwrap();
        for selected in [None, Some(context.clone())] {
            let (project, response) = manager.dispatch_sync(selected.clone(), Req::Ping);
            assert!(project.is_none());
            assert!(response.ok && response.data["host_ready"] == true);
            let (project, response) = manager.dispatch_sync(
                selected,
                Req::Ceo {
                    worker_id: String::new(),
                    token: String::new(),
                    command: CeoCommand::Status,
                },
            );
            assert!(project.is_none());
            assert!(response.ok);
        }
        let (project, response) = manager.dispatch_sync(Some(context.clone()), Req::StatusAll);
        assert!(project.is_none());
        assert!(!response.ok);
        let (_, shutdown) = tokio::sync::watch::channel(false);
        for request in [
            Req::DirectoryAll,
            Req::FederationDirectory {
                machine: None,
                project: None,
            },
            Req::RouteResolveNative {
                session_id: "missing".into(),
                native_thread_id: None,
            },
        ] {
            let (project, _) =
                dispatch_wire_routed(manager.clone(), None, request, shutdown.clone()).await;
            assert!(project.is_none());
        }
        assert!(manager.routes.lock().unwrap().is_empty());
        manager
            .reserve_external_route(
                &context,
                &manager.storage_root_for_new(
                    &root,
                    context.project_scope.as_str(),
                    context.app_scope_id.as_str(),
                ),
            )
            .unwrap();
        let expected = manager.select_runtime(&context).unwrap();
        let transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/unused-dispatch-status.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("dispatch-status-session".into()),
            thread_id: Some("dispatch-status-thread".into()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec![],
            self_check: "static non-tmux fixture".into(),
        };
        peer_tests::seed_committed_owner(
            &expected,
            "dispatch-status-worker",
            "dispatch-status-token",
            &transport,
            &context.canonical_root,
            Some(context.app_scope_id.clone()),
        );
        let (project, response) =
            dispatch_wire_routed(manager.clone(), Some(context), Req::StatusAll, shutdown).await;
        assert!(response.ok, "{response:?}");
        assert!(Arc::ptr_eq(project.as_ref().unwrap(), &expected));
        assert_eq!(std::fs::read(&host.state_path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cross_project_dispatch_does_not_lock_host_state() {
        use super::super::*;
        let (server, root) = peer_tests::test_server();
        let host = Arc::new(server);
        let manager = ProjectRuntimeManager::new(host.clone(), &host.host_paths).unwrap();
        let mut contexts = Vec::new();
        for name in ["source", "target"] {
            let project = root.join(name);
            std::fs::create_dir_all(project.join(".agent-collab")).unwrap();
            let context = ProjectContext {
                project_scope: GlobalState::canonical_project_scope(&project).unwrap(),
                app_scope_id: crate::identity::AppServerId::new(crate::identity::CLI_APP_SERVER_ID)
                    .unwrap(),
                canonical_root: project
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                runtime_context: None,
            };
            manager
                .reserve_external_route(
                    &context,
                    &manager.storage_root_for_new(
                        &project,
                        context.project_scope.as_str(),
                        context.app_scope_id.as_str(),
                    ),
                )
                .unwrap();
            contexts.push(context);
        }
        let source = manager.select_runtime(&contexts[0]).unwrap();
        let target = manager.select_runtime(&contexts[1]).unwrap();
        let transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/dispatch-no-network-fixture.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("source-session".into()),
            thread_id: Some("source-thread".into()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec![],
            self_check: "static non-tmux fixture".into(),
        };
        peer_tests::seed_committed_owner(
            &source,
            "source-worker",
            "source-token",
            &transport,
            &contexts[0].canonical_root,
            Some(contexts[0].app_scope_id.clone()),
        );
        let binding = manager
            .binding_control_store()
            .load()
            .unwrap()
            .active_values()
            .find(|slot| slot.binding.agent_id.as_str() == "source-worker")
            .unwrap()
            .binding
            .clone();
        let command: CommandEnvelope = serde_json::from_value(json!({
            "command_id":"dispatch-command", "operation_id":"dispatch-operation",
            "actor_binding_id":binding.binding_id, "actor_runtime_id":binding.runtime_id,
            "scope":binding.route_scope()
        }))
        .unwrap();
        // A bad token still exercises source authentication inside the project
        // lock boundary, without any notification or external transport call.
        let held = host.state.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let dispatching = manager.clone();
        let context = contexts[1].clone();
        let task = std::thread::spawn(move || {
            let result = dispatching.dispatch_cross_project_send(
                &context,
                Req::CrossProjectSend {
                    from: "ignored".into(),
                    from_project: "ignored".into(),
                    worker_id: "source-worker".into(),
                    token: "bad-token".into(),
                    command: Some(command),
                    to: "target-worker".into(),
                    subject: "test".into(),
                    body: "test".into(),
                    in_reply_to: None,
                    reply_policy: None,
                    target_session: None,
                },
            );
            tx.send(result.1).unwrap();
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(2));
        drop(held);
        task.join().unwrap();
        let response = result.expect("project authentication must not wait for host State");
        assert!(!response.ok);
        assert!(response.error.unwrap().contains("token mismatch"));
        assert!(target
            .state
            .lock()
            .unwrap()
            .inbox_of("target-worker")
            .is_empty());
        let mut target_transport = transport.clone();
        target_transport.session_id = Some("target-session".into());
        target_transport.thread_id = Some("target-thread".into());
        peer_tests::seed_committed_owner(
            &target,
            "target-worker",
            "target-token",
            &target_transport,
            &contexts[1].canonical_root,
            Some(contexts[1].app_scope_id.clone()),
        );
        let before = std::fs::read(&host.state_path).unwrap();
        let held = host.state.lock().unwrap();
        let map = manager.binding_control_store().load().unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let (tx, rx) = std::sync::mpsc::channel();
        let mut tasks = Vec::new();
        for (sender, token, recipient, destination) in [
            (
                "source-worker",
                "source-token",
                "target-worker",
                contexts[1].clone(),
            ),
            (
                "target-worker",
                "target-token",
                "source-worker",
                contexts[0].clone(),
            ),
        ] {
            let binding = &map
                .active_values()
                .find(|slot| slot.binding.agent_id.as_str() == sender)
                .unwrap()
                .binding;
            let command = serde_json::from_value(json!({
                "command_id":format!("{sender}-command"), "operation_id":format!("{sender}-operation"),
                "actor_binding_id":binding.binding_id, "scope":binding.route_scope()
            })).unwrap();
            let manager = manager.clone();
            let barrier = barrier.clone();
            let tx = tx.clone();
            tasks.push(std::thread::spawn(move || {
                barrier.wait();
                tx.send(manager.dispatch_cross_project_send(
                    &destination,
                    Req::CrossProjectSend {
                        from: "forged compatibility sender".into(),
                        from_project: "ignored".into(),
                        worker_id: sender.into(),
                        token: token.into(),
                        command: Some(command),
                        to: recipient.into(),
                        subject: "opposing send".into(),
                        body: sender.into(),
                        in_reply_to: None,
                        reply_policy: None,
                        target_session: None,
                    },
                ))
                .unwrap();
            }));
        }
        let results = (
            rx.recv_timeout(std::time::Duration::from_secs(2)),
            rx.recv_timeout(std::time::Duration::from_secs(2)),
        );
        drop(held);
        for task in tasks {
            task.join().unwrap();
        }
        for result in [results.0, results.1] {
            let (runtime, response) =
                result.expect("opposing project sends must not wait for host State");
            assert!(response.ok, "{response:?}");
            assert_eq!(response.data["consumed"], false);
            let runtime = runtime.unwrap();
            let persisted = current_state::load(&runtime.state_path, false).unwrap();
            let id = response.data["msg_id"].as_str().unwrap();
            assert!(
                persisted.msgs.contains_key(id),
                "send response must follow the target mailbox commit"
            );
            assert!(!host.state.lock().unwrap().msgs.contains_key(id));
        }
        assert_eq!(std::fs::read(&host.state_path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_map_routes_without_legacy_host_publication() {
        use super::super::*;
        let (server, root) = peer_tests::test_server();
        let host = Arc::new(server);
        let manager = ProjectRuntimeManager::new(host.clone(), &host.host_paths).unwrap();
        let context = ProjectContext {
            project_scope: GlobalState::canonical_project_scope(&root).unwrap(),
            app_scope_id: crate::identity::AppServerId::new(crate::identity::CLI_APP_SERVER_ID)
                .unwrap(),
            canonical_root: root.canonicalize().unwrap().to_string_lossy().into_owned(),
            runtime_context: None,
        };
        manager
            .reserve_external_route(
                &context,
                &manager.storage_root_for_new(
                    &root,
                    context.project_scope.as_str(),
                    context.app_scope_id.as_str(),
                ),
            )
            .unwrap();
        let runtime = manager.select_runtime(&context).unwrap();
        let transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/current-map-route-fixture.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("current-map-session".into()),
            thread_id: Some("current-map-thread".into()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec![],
            self_check: "isolated committed fixture".into(),
        };
        peer_tests::seed_committed_owner(
            &runtime,
            "mapped-worker",
            "mapped-token",
            &transport,
            &context.canonical_root,
            Some(context.app_scope_id.clone()),
        );
        let map = manager.binding_control_store().load().unwrap();
        let binding = map.active_values().next().unwrap().binding.clone();
        let before = std::fs::read(&host.state_path).unwrap();
        let route = manager
            .resolve_route_by_native_session(&context, "current-map-session")
            .expect("current identity-map must route without a host index");
        assert_eq!(route.agent_id, binding.agent_id);
        assert!(manager.binding_publication_matches(&binding).unwrap());
        manager.reconcile_binding_publication(&binding).unwrap();
        manager.reconcile_controlled_thread_routes(&map).unwrap();
        assert_eq!(
            std::fs::read(&host.state_path).unwrap(),
            before,
            "current route validation must not publish a legacy host index"
        );
        let slot = map
            .current_for_worker(
                &binding.project_scope,
                &binding.app_scope_id,
                binding.agent_id.as_str(),
            )
            .unwrap()
            .unwrap();
        let effect = binding_slot::PendingEffect {
            kind: binding_slot::PendingEffectKind::HumanRetire {
                binding_id: binding.binding_id.clone(),
                generation: slot.generation(),
            },
            role: None,
            effect_id: "map-retirement-effect".into(),
            operation_id: "map-retirement-operation".into(),
            project_scope: binding.project_scope.clone(),
            app_scope_id: binding.app_scope_id.clone(),
            name: slot.identity_key.name.clone(),
            previous_worker_id: Some(binding.agent_id.to_string()),
            previous_session_id: Some("current-map-session".into()),
            target_worker_id: binding.agent_id.to_string(),
            target_session_id: "current-map-session".into(),
            occupancy_revision: 1,
            source_names: vec![],
            moves_existing_names: false,
            pane_anchor: None,
            created_ms: now_ms(),
        };
        manager
            .commit_control_change(binding_store::ControlChange::UnregisterWithEffect {
                key: slot.identity_key.clone(),
                effect: effect.clone(),
            })
            .unwrap();
        // Retirement is already authoritative while its project effects wait.
        assert!(manager
            .resolve_route_by_native_session(&context, "current-map-session")
            .is_err());
        assert!(!manager.binding_publication_matches(&binding).unwrap());
        manager.recover_pending_effects().unwrap();
        let command = "retire_name:map-retirement-operation";
        let receipt = runtime.state.lock().unwrap().global.command_receipts[command].clone();
        assert_eq!(receipt.operation_id.as_str(), effect.operation_id);
        assert!(!manager
            .host_control
            .state
            .lock()
            .unwrap()
            .commands
            .receipts
            .contains_key(command));
        assert!(manager
            .binding_control_store()
            .load()
            .unwrap()
            .pending_effect(&effect.effect_id)
            .is_none());
        manager.recover_pending_effects().unwrap();
        assert_eq!(
            runtime.state.lock().unwrap().global.command_receipts[command],
            receipt
        );
        assert_eq!(std::fs::read(&host.state_path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reset_owner_upgrade_preserves_control_ledger_and_does_not_reimport() {
        let (server, root) = super::super::peer_tests::test_server();
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        let command = owner
            .commit_command_locked(
                &mut owner.state.lock().unwrap(),
                "control-command",
                "control-operation",
                &[],
                serde_json::json!({"original":true}),
                None,
            )
            .unwrap();
        let mut file: Value = serde_json::from_slice(&std::fs::read(&owner.path).unwrap()).unwrap();
        file["state"]["reset_owner_initialized"] = serde_json::json!(true);
        file["state"]["legacy_migration_completed"] = serde_json::json!(true);
        file["state"]["pending_project_resets"] = serde_json::json!({"fixed-project":true});
        std::fs::write(&owner.path, serde_json::to_vec(&file).unwrap()).unwrap();
        // Reopening the typed owner must preserve its authoritative versions
        // and replay outcome while retiring only the actual reset intent.
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        {
            let state = owner.state.lock().unwrap();
            assert!(state.pending_project_resets.contains_key("fixed-project"));
            assert_eq!(state.commands.sequence, command.receipt.sequence);
            assert_eq!(state.commands.revision, command.receipt.revision);
            assert_eq!(
                state.commands.receipts["control-command"].outcome,
                serde_json::json!({"original":true})
            );
        }
        let store =
            super::super::binding_store::BindingControlStore::new(server.host_paths.state_root());
        owner.handoff_pending_resets(&store).unwrap();
        let retired: Value = serde_json::from_slice(&std::fs::read(&owner.path).unwrap()).unwrap();
        assert!(retired["state"].get("reset_owner_initialized").is_none());
        assert!(retired["state"].get("legacy_migration_completed").is_none());
        assert!(store
            .load()
            .unwrap()
            .pending_resets
            .contains(&"fixed-project".to_owned()));
        store.complete_reset("fixed-project").unwrap();
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        let mut state = owner.state.lock().unwrap();
        assert!(
            state.pending_project_resets.is_empty(),
            "stale cleanup source must not resurrect reset"
        );
        let replay = owner
            .commit_command_locked(
                &mut state,
                "control-command",
                "control-operation",
                &[],
                Value::Null,
                Some(0),
            )
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.receipt, command.receipt);
        assert_eq!(replay.outcome, command.outcome);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn host_commands_reopen_replay_and_preserve_typed_versions() {
        let (server, root) = super::super::peer_tests::test_server();
        let mut legacy = super::super::State::default();
        legacy.sequence = 12;
        legacy.revision = 9;
        legacy.sync_global_version();
        let original = legacy
            .global
            .record_command(
                CommandId::new("legacy-command").unwrap(),
                OperationId::new("legacy-operation").unwrap(),
                serde_json::json!({"accepted":true}),
            )
            .unwrap();
        legacy.sequence = legacy.global.sequence;
        legacy.revision = legacy.global.revision;
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        assert!(owner.state.lock().unwrap().commands.receipts.is_empty());
        // An already-published typed stream is authoritative. This fixture
        // writes that exact resource; no project import path exists.
        {
            let mut typed = owner.state.lock().unwrap();
            typed.commands.epoch = original.epoch;
            typed.commands.sequence = original.sequence;
            typed.commands.revision = original.revision;
            typed
                .commands
                .receipts
                .insert("legacy-command".into(), original.clone());
            assert!(owner.save(&typed).unwrap());
        }
        drop(owner);
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        let mut state = owner.state.lock().unwrap();
        let replay = owner
            .commit_command_locked(
                &mut state,
                "legacy-command",
                "legacy-operation",
                &[HostChange::CeoProgressRecorded {
                    project: "must-not-apply".into(),
                    content: CeoProgressContent::new("subject", "body"),
                    urgent: true,
                }],
                Value::Null,
                Some(0),
            )
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.receipt.sequence, original.sequence);
        assert_eq!(replay.receipt.revision, original.revision);
        assert_eq!(replay.outcome, original.outcome);
        assert!(state.ceo_report_state.projects.is_empty());
        let before = state.clone();
        assert!(owner
            .commit_command_locked(
                &mut state,
                "legacy-command",
                "other-operation",
                &[],
                Value::Null,
                None
            )
            .is_err());
        assert!(owner
            .commit_command_locked(
                &mut state,
                "other-command",
                "legacy-operation",
                &[],
                Value::Null,
                None
            )
            .is_err());
        assert!(owner
            .commit_command_locked(
                &mut state,
                "other-command",
                "other-operation",
                &[],
                Value::Null,
                Some(0)
            )
            .is_err());
        assert_eq!(*state, before);
        let next = owner
            .commit_command_locked(
                &mut state,
                "next-command",
                "next-operation",
                &[],
                Value::Null,
                None,
            )
            .unwrap();
        assert_eq!(next.receipt.sequence, original.sequence + 1);
        assert_eq!(next.receipt.revision, original.revision + 1);
        assert_eq!(state.commands.epoch, original.epoch);
        drop(state);
        let reopened = HostControl::read_current(&server.host_paths).unwrap();
        assert!(reopened
            .state
            .lock()
            .unwrap()
            .commands
            .receipts
            .contains_key("next-command"));
        assert_eq!(legacy.global.command_receipts.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reset_handoff_failures_preserve_intent_before_cleanup_and_do_not_resurrect() {
        use super::super::binding_store::BindingControlStore;
        let (server, root) = super::super::peer_tests::test_server();
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        let mut typed = HostControlState::default();
        typed
            .pending_project_resets
            .insert("fixture-scope".into(), true);
        assert!(owner.save(&typed).unwrap());
        let bytes = std::fs::read(&owner.path).unwrap();
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        let store = BindingControlStore::new(server.host_paths.state_root());
        let map_path = server.host_paths.state_root().join("identity-map.json");
        std::fs::create_dir(&map_path).unwrap();
        assert!(owner.handoff_pending_resets(&store).is_err());
        assert_eq!(std::fs::read(&owner.path).unwrap(), bytes);
        assert!(owner
            .state
            .lock()
            .unwrap()
            .pending_project_resets
            .contains_key("fixture-scope"));
        std::fs::remove_dir(&map_path).unwrap();
        std::fs::remove_file(&owner.path).unwrap();
        std::fs::create_dir(&owner.path).unwrap();
        assert!(owner.handoff_pending_resets(&store).is_err());
        assert!(store
            .load()
            .unwrap()
            .pending_resets
            .contains(&"fixture-scope".into()));
        assert!(owner
            .state
            .lock()
            .unwrap()
            .pending_project_resets
            .contains_key("fixture-scope"));
        // The failed old-owner retirement must finish before cleanup can run.
        std::fs::remove_dir(&owner.path).unwrap();
        std::fs::write(&owner.path, &bytes).unwrap();
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        owner.handoff_pending_resets(&store).unwrap();
        assert!(owner
            .state
            .lock()
            .unwrap()
            .pending_project_resets
            .is_empty());
        store.complete_reset("fixture-scope").unwrap();
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        owner.handoff_pending_resets(&store).unwrap();
        assert!(store.load().unwrap().pending_resets.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn host_failed_command_does_not_publish_state_or_receipt() {
        let (server, root) = super::super::peer_tests::test_server();
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        let mut state = owner.state.lock().unwrap();
        let before = state.clone();
        assert!(!owner.path.exists());
        std::fs::create_dir_all(&owner.path).unwrap();
        let error = owner
            .commit_command_locked(
                &mut state,
                "failed-command",
                "failed-operation",
                &[HostChange::CeoProgressRecorded {
                    project: "project".into(),
                    content: CeoProgressContent::new("subject", "body"),
                    urgent: false,
                }],
                Value::Null,
                None,
            )
            .unwrap_err();
        assert!(matches!(error, MutationError::Persistence(_)));
        assert_eq!(*state, before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn host_control_reopen_rejects_business_fields_and_keeps_current_authority() {
        let (server, root) = super::super::peer_tests::test_server();
        let owner = HostControl::read_current(&server.host_paths).unwrap();
        let grant = CeoGrant {
            workspace: root.to_string_lossy().into_owned(),
            app_scope_id: "appserver-cli".into(),
            name: "Alice".into(),
            granted_by: "user".into(),
            approval: "approved".into(),
            granted_ms: 1,
        };
        owner
            .commit_checked(&[HostChange::CeoGrantSet {
                grant: Some(grant.clone()),
            }])
            .unwrap();
        let reopened = HostControl::read_current(&server.host_paths).unwrap();
        assert_eq!(
            reopened.state.lock().unwrap().ceo_grant.as_ref(),
            Some(&grant)
        );
        let mut file: Value = serde_json::from_slice(&std::fs::read(&owner.path).unwrap()).unwrap();
        file["state"]["workers"] = serde_json::json!({});
        std::fs::write(&owner.path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(HostControl::read_current(&server.host_paths)
            .err()
            .unwrap()
            .starts_with("HOST_CONTROL_DECODE_FAILED"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostControlState {
    #[serde(default)]
    pub(super) pending_project_resets: BTreeMap<String, bool>,
    pub(super) ceo_grant: Option<CeoGrant>,
    pub(super) ceo_bootstraps: HashMap<String, CeoBootstrapRecord>,
    pub(super) ceo_report_state: CeoReportState,
    pub(super) commands: HostCommandLedger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct HostCommandLedger {
    pub(super) epoch: u64,
    pub(super) sequence: u64,
    pub(super) revision: u64,
    pub(super) receipts: BTreeMap<String, CommandReceipt>,
}

impl Default for HostCommandLedger {
    fn default() -> Self {
        Self {
            epoch: super::global_state::INITIAL_EPOCH,
            sequence: 0,
            revision: 0,
            receipts: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlFile {
    schema_version: u32,
    state: HostControlState,
}

pub(super) struct HostControl {
    pub(super) state: Mutex<HostControlState>,
    path: PathBuf,
}

impl HostControl {
    /// Publish each actual old intent to its sole ongoing owner before
    /// retiring the old copy. Cleanup may only start after this succeeds.
    pub(super) fn handoff_pending_resets(
        &self,
        store: &super::binding_store::BindingControlStore,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.pending_project_resets.is_empty() {
            return Ok(());
        }
        for scope in state.pending_project_resets.keys() {
            store.begin_reset(scope)?;
        }
        let mut candidate = state.clone();
        candidate.pending_project_resets.clear();
        if !self.save(&candidate).map_err(|e| e.to_string())? {
            return Err(
                MutationError::DurabilityUnknown(self.path.display().to_string()).to_string(),
            );
        }
        *state = candidate;
        Ok(())
    }

    /// A read-only current view. Project helpers never import project State
    /// or initialize host authority when the owner resource is unavailable.
    pub(super) fn read_current(host_paths: &crate::scope::HostPaths) -> Result<Self, String> {
        let path = host_paths.state_root().join("host-control.json");
        let body = match std::fs::read(&path) {
            Ok(body) => body,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    path,
                    state: Mutex::new(HostControlState::default()),
                });
            }
            Err(error) => return Err(format!("HOST_CONTROL_READ_FAILED: {error}")),
        };
        let mut file: Value = serde_json::from_slice(&body)
            .map_err(|e| format!("HOST_CONTROL_DECODE_FAILED: {e}"))?;
        // Accept obsolete flags from the actual typed owner without keeping
        // them in current state or consulting any project resource.
        if let Some(state) = file.get_mut("state").and_then(Value::as_object_mut) {
            state.remove("reset_owner_initialized");
            state.remove("legacy_migration_completed");
        }
        let file: ControlFile =
            serde_json::from_value(file).map_err(|e| format!("HOST_CONTROL_DECODE_FAILED: {e}"))?;
        if file.schema_version != 1 {
            return Err(format!(
                "HOST_CONTROL_VERSION_UNSUPPORTED: {}",
                file.schema_version
            ));
        }
        Self::validate(&file.state).map_err(|e| e.to_string())?;
        Ok(Self {
            path,
            state: Mutex::new(file.state),
        })
    }

    fn validate(state: &HostControlState) -> Result<(), MutationError> {
        let commands = &state.commands;
        if commands.epoch == 0 {
            return Err(MutationError::InvalidState(
                "host epoch must be nonzero".into(),
            ));
        }
        let mut operations = BTreeSet::new();
        for (key, receipt) in &commands.receipts {
            receipt
                .validate()
                .map_err(|e| MutationError::InvalidState(e.to_string()))?;
            if key != receipt.command_id.as_str()
                || receipt.epoch != commands.epoch
                || !operations.insert(receipt.operation_id.as_str())
            {
                return Err(MutationError::InvalidState(
                    "invalid host command ledger".into(),
                ));
            }
        }
        Ok(())
    }

    fn save(&self, state: &HostControlState) -> Result<bool, MutationError> {
        #[derive(Serialize)]
        struct ControlRef<'a> {
            schema_version: u32,
            state: &'a HostControlState,
        }
        let body = serde_json::to_vec(&ControlRef {
            schema_version: 1,
            state,
        })
        .map_err(|e| MutationError::Persistence(e.to_string()))?;
        super::current_state::save_bytes(&self.path, &body)
    }

    fn apply(state: &mut HostControlState, change: &HostChange) -> Result<(), MutationError> {
        match change {
            HostChange::CeoGrantSet { grant } => {
                state.ceo_grant = grant.clone();
                state.ceo_report_state.notice_gate = Default::default();
            }
            HostChange::CeoBootstrapSet { record } => {
                state
                    .ceo_bootstraps
                    .insert(record.operation_id.clone(), record.clone());
            }
            HostChange::CeoProgressRecorded {
                project,
                content,
                urgent,
            } => {
                state
                    .ceo_report_state
                    .record_progress(project.clone(), content.clone(), *urgent);
            }
            HostChange::CeoReportBatchPrepared {
                batch_id,
                coverage,
                urgent,
            } => {
                state
                    .ceo_report_state
                    .prepare_batch(batch_id.clone(), coverage.clone(), *urgent)
                    .map_err(|e| MutationError::InvalidState(e.to_string()))?;
            }
            HostChange::CeoReportBatchAcknowledged {
                batch_id,
                reported_ms,
            } => {
                state
                    .ceo_report_state
                    .acknowledge_batch(batch_id, *reported_ms)
                    .map_err(|e| MutationError::InvalidState(e.to_string()))?;
            }
            HostChange::CeoReportNoticeGateUpdated { gate } => {
                state.ceo_report_state.notice_gate = gate.clone();
            }
        }
        Ok(())
    }

    pub(super) fn commit_checked(
        &self,
        changes: &[HostChange],
    ) -> Result<CommitReceipt, MutationError> {
        self.commit_locked_checked(&mut self.state.lock().unwrap(), changes)
    }

    pub(super) fn commit_locked_checked(
        &self,
        state: &mut HostControlState,
        changes: &[HostChange],
    ) -> Result<CommitReceipt, MutationError> {
        if changes.is_empty() {
            return Ok(CommitReceipt {
                sequence: state.commands.sequence,
                revision: state.commands.revision,
            });
        }
        let candidate = Self::candidate(state, changes)?;
        self.publish(state, candidate)
    }

    fn candidate(
        state: &HostControlState,
        changes: &[HostChange],
    ) -> Result<HostControlState, MutationError> {
        let mut candidate = state.clone();
        for change in changes {
            Self::apply(&mut candidate, change)?;
        }
        candidate.commands.sequence = candidate
            .commands
            .sequence
            .checked_add(1)
            .ok_or_else(|| MutationError::InvalidState("sequence counter overflow".into()))?;
        candidate.commands.revision = candidate
            .commands
            .revision
            .checked_add(1)
            .ok_or_else(|| MutationError::InvalidState("revision counter overflow".into()))?;
        Ok(candidate)
    }

    fn publish(
        &self,
        state: &mut HostControlState,
        candidate: HostControlState,
    ) -> Result<CommitReceipt, MutationError> {
        Self::validate(&candidate)?;
        let confirmed = self.save(&candidate)?;
        let receipt = CommitReceipt {
            sequence: candidate.commands.sequence,
            revision: candidate.commands.revision,
        };
        *state = candidate;
        if !confirmed {
            return Err(MutationError::DurabilityUnknown(
                self.path.display().to_string(),
            ));
        }
        Ok(receipt)
    }

    pub(super) fn commit_command_locked(
        &self,
        state: &mut HostControlState,
        command: &str,
        operation: &str,
        changes: &[HostChange],
        outcome: Value,
        expected_revision: Option<u64>,
    ) -> Result<CommandOutcome, MutationError> {
        let command_id = CommandId::new(command.to_owned())
            .map_err(|e| MutationError::InvalidCommand(e.to_string()))?;
        let operation_id = OperationId::new(operation.to_owned())
            .map_err(|e| MutationError::InvalidCommand(e.to_string()))?;
        if let Some(receipt) = state.commands.receipts.get(command) {
            if receipt.operation_id != operation_id {
                return Err(MutationError::InvalidCommand(format!(
                    "command {command} already belongs to another operation"
                )));
            }
            return Ok(CommandOutcome {
                receipt: CommitReceipt {
                    sequence: receipt.sequence,
                    revision: receipt.revision,
                },
                operation_id: operation.into(),
                outcome: receipt.outcome.clone(),
                replayed: true,
            });
        }
        if state
            .commands
            .receipts
            .values()
            .any(|receipt| receipt.operation_id == operation_id)
        {
            return Err(MutationError::InvalidCommand(format!(
                "operation {operation} already belongs to another command"
            )));
        }
        if expected_revision.is_some_and(|revision| revision != state.commands.revision) {
            return Err(MutationError::InvalidCommand(format!(
                "CAS_MISMATCH: expected {}, observed {}",
                expected_revision.unwrap(),
                state.commands.revision
            )));
        }
        let mut candidate = Self::candidate(state, changes)?;
        candidate.commands.receipts.insert(
            command.into(),
            CommandReceipt {
                command_id,
                operation_id,
                epoch: candidate.commands.epoch,
                sequence: candidate.commands.sequence,
                revision: candidate.commands.revision,
                outcome: outcome.clone(),
            },
        );
        let receipt = self.publish(state, candidate)?;
        Ok(CommandOutcome {
            receipt,
            operation_id: operation.into(),
            outcome,
            replayed: false,
        })
    }
}
