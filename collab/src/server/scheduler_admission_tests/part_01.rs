    use super::*;
    use crate::server::peer_tests::{
        register, register_appserver_worker, test_appserver_transport, test_server,
    };
    use crate::server::state::MasterWakeSignal;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Condvar, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    // Notification/recovery tests use the retained private-child dispatcher.
    // Public ordinary-peer dispatch is tested as an explicit board-only gate.
    fn register_private_dispatch_peer(server: &Server) {
        register_appserver_worker(server, &server.root, "peer", "thread-peer");
        server.commit(&[Event::SubagentUpdated { subagent: crate::subagent::Record {
            id: "managed-peer".into(), parent: "master".into(), peer: "peer".into(),
            status: "idle".into(), thread_id: None, profile: None,
            created_ms: now_ms(), ready_deadline_ms: now_ms() + 60_000,
            last_message: None, error: None, probe_failures: vec![], runtime: Some("codex".into()),
            create_operation_id: None,
            binding_id: None,
        } }]);
    }

    fn promote_master(server: &Server) {
        crate::server::peer_tests::promote_master(server, "master", "scheduler test");
    }

    fn private_peer_context(server: &Server) -> ProjectContext {
        let state = server.state.lock().unwrap();
        let binding = state.global.projects.values().flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == "peer").unwrap();
        ProjectContext::for_registered_route(&server.root, &crate::identity::RuntimeIdentity {
            agent_id: binding.agent_id.clone(), runtime_id: binding.runtime_id.clone(),
            appserver_id: binding.app_scope_id.clone(), binding_id: binding.binding_id.clone(),
            session_id: binding.session_id.clone(), native_thread_id: binding.native_thread_id.clone(),
        }).unwrap()
    }

    // Current persisted reservations still have retry, visibility and Working
    // readers. This fixture does not simulate the removed admission logger.
    fn seed_private_reservation(server: &Server, request_id: &str, status: &str) {
        let message_id = format!("scheduler-{request_id}");
        let now = now_ms();
        let mut child = server.state.lock().unwrap().subagents["managed-peer"].clone();
        child.status = "assigned".into();
        child.thread_id = server.state.lock().unwrap().workers["peer"]
            .transport.as_ref().unwrap().thread_id.clone();
        child.last_message = Some(message_id.clone());
        server.commit(&[
            Event::Sent { msg: Message {
                id: message_id.clone(), from: "master".into(), to: "peer".into(),
                mtype: "notify".into(), subject: Some("Current reservation".into()),
                body: "Preserve the original payload".into(), in_reply_to: Some("source-message".into()),
                created_ms: now, state: "pending".into(), wake_attempt_count: 0,
                last_wake_attempt_ms: 0, retry_attempted: false,
            } },
            Event::TaskCreated { task: TaskRec {
                id: format!("task-{message_id}"), owner: "peer".into(), created_by: "master".into(),
                feature_id: Some("current-feature".into()), worktree_path: None, branch: None,
                base_commit: None, priority: "p1".into(), status: "assigned".into(),
                next_step: Some("accept".into()), wait: None, isolation_mode: None,
                created_ms: now, updated_ms: now,
            } },
            Event::SubagentUpdated { subagent: child },
            Event::DeliveryMode { msg_id: message_id.clone(), mode: "explicit-notification".into(), source_thread_id: None },
            Event::WakeBound { message_id: message_id.clone(), subscription_id: "sub-default-direct-message-peer".into() },
            Event::SchedulerAdmission { admission: crate::server::state::SchedulerAdmissionRecord {
                request_id: request_id.into(), decision: "reuse-idle-managed-subagent".into(),
                worker_id: "peer".into(), managed_subagent_id: Some("managed-peer".into()),
                message_id: message_id.clone(), task_id: format!("task-{message_id}"),
                status: status.into(), error: (status == "failed").then(|| "stored reservation failed".into()),
                created_ms: now, updated_ms: now,
            } },
        ]);
        *server.state.lock().unwrap() = current_state::load(&server.state_path, false).unwrap();
    }

    fn idle_projection_task(id: &str, owner: &str, status: &str) -> TaskRec {
        TaskRec {
            id: id.into(),
            owner: owner.into(),
            created_by: "master".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p1".into(),
            status: status.into(),
            next_step: None,
            wait: None,
            isolation_mode: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        }
    }

    fn seed_live_observed_idle(server: &Server, root: &std::path::Path, worker_id: &str) {
        register_appserver_worker(server, root, worker_id, &format!("thread-{worker_id}"));
        server.commit(&[
            Event::KeepaliveUpdated {
                worker_id: worker_id.into(),
                record: crate::server::keepalive::Record {
                    observed: "idle".into(),
                    ..Default::default()
                },
            },
            Event::MasterWakeSignal {
                signal: MasterWakeSignal::WorkerIdle {
                    worker_id: worker_id.into(),
                },
                at_ms: now_ms(),
            },
        ]);
    }

    fn idle_projection_subagent(id: &str, peer: &str, status: &str) -> crate::subagent::Record {
        crate::subagent::Record {
            id: id.into(),
            parent: "master".into(),
            peer: peer.into(),
            status: status.into(),
            thread_id: Some(format!("thread-{peer}")),
            profile: None,
            created_ms: now_ms(),
            ready_deadline_ms: now_ms() + 60_000,
            last_message: None,
            error: None,
            probe_failures: vec![],
            runtime: Some("codex".into()),
            create_operation_id: None,
            binding_id: None,
        }
    }

    fn idle_keys(value: &serde_json::Value) -> Vec<String> {
        value["idle_workers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn authenticated_start_is_unsupported_without_mutation() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "idle-peer", "%idle-peer");
        promote_master(&server);
        let server = Arc::new(server);

        let start = |id: Option<&str>, token: &str| {
            dispatch(
                &server,
                Req::Subagent {
                    worker_id: "master".into(),
                    token: token.into(),
                    command: crate::subagent::Action::Start {
                        id: id.map(str::to_owned),
                        runtime: Some("codex".into()),
                    },
                    launch_env: Default::default(),
                },
            )
        };
        let first = start(None, "token-master");
        assert!(!first.ok, "tmux cannot create a managed Codex thread");
        assert!(first
            .error
            .as_deref()
            .unwrap()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));
        let second = start(None, "token-master");
        assert!(!second.ok, "repeated Start stays explicitly unsupported");
        let direct = crate::subagent::handle_with_env(
            &server,
            "master",
            "token-master",
            crate::subagent::Action::Start {
                id: None,
                runtime: Some("codex".into()),
            },
            Default::default(),
        );
        assert!(!direct.ok, "direct Start stays explicitly unsupported");
        let state = server.state.lock().unwrap();
        assert!(state.subagents.is_empty());
        assert!(state.tasks.is_empty());
        assert!(state.msgs.is_empty());
        assert!(state.scheduler_admissions.is_empty());
        drop(state);

        let denied = start(None, "wrong-token");
        assert!(!denied.ok);
        assert!(denied.error.unwrap().contains("authentication failed"));
        let state = server.state.lock().unwrap();
        assert!(state.subagents.is_empty());
        assert!(state.tasks.is_empty());
        assert!(state.msgs.is_empty());
        assert!(state.scheduler_admissions.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_admission_rejects_ambiguous_server_route_scope() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "idle-peer", "%idle-peer");
        promote_master(&server);
        let route_scope = {
            let state = server.state.lock().unwrap();
            let binding = state
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values())
                .find(|binding| binding.agent_id.as_str() == "master")
                .unwrap();
            binding.route_scope()
        };
        let second_app = crate::identity::AppServerId::new("tui-second").unwrap();
        let registration = crate::server::global_state::ProjectRegistration::new(
            route_scope.project_scope_id.clone(),
            second_app,
        )
        .unwrap();
        let mut second_binding = {
            let state = server.state.lock().unwrap();
            state
                .global
                .lookup_binding_for(
                    &route_scope,
                    &crate::identity::BindingId::new("binding-master").unwrap(),
                )
                .unwrap()
                .clone()
        };
        second_binding.app_scope_id = registration.app_scope_id.clone();
        second_binding.binding_id = crate::identity::BindingId::new("binding-second").unwrap();
        second_binding.runtime_id = crate::identity::RuntimeId::new("runtime-second").unwrap();
        server.commit(&[
            Event::GlobalProjectRegistered { registration },
            Event::GlobalRuntimeBound {
                binding: second_binding,
            },
        ]);

        let admission =
            scheduler_admit_subagent_start(&server, "master", "token-master", None, Some("codex"));
        let error = admission
            .expect_err("ambiguous route must not authorize scheduler admission")
            .error
            .unwrap_or_default();
        assert!(error.contains("MANAGED_SUBAGENT_UNSUPPORTED"), "{error}");

        let server = Arc::new(server);
        let dispatched = dispatch(
            &server,
            Req::Subagent {
                worker_id: "master".into(),
                token: "token-master".into(),
                command: crate::subagent::Action::Dispatch {
                    request_id: "req-ambiguous".into(),
                    subject: "subject".into(),
                    body: "body".into(),
                    feature_id: None,
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p1".into(),
                    next_step: None,
                },
                launch_env: Default::default(),
            },
        );
        assert!(!dispatched.ok);
        assert!(dispatched
            .error
            .unwrap_or_default()
            .contains("scheduler dispatch requires a unique route scope"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_excludes_active_unknown_and_managed_capacity() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "peer", "%peer");
        server.commit(&[Event::TaskCreated {
            task: TaskRec {
                id: "active-peer-task".into(),
                owner: "peer".into(),
                created_by: "master".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p1".into(),
                status: "assigned".into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        }]);
        assert!(registered_available_peer_for_admission(&server, "master").is_none());
        std::fs::remove_dir_all(root).unwrap();

        let (server, root) = test_server();
        register(&server, "master", "%master");
        register_appserver_worker(&server, &root, "managed-peer", "thread-managed-peer");
        server.commit(&[Event::SubagentUpdated {
            subagent: crate::subagent::Record {
                id: "existing-child".into(),
                parent: "master".into(),
                peer: "managed-peer".into(),
                status: "idle".into(),
                thread_id: Some("thread-managed-peer".into()),
                profile: None,
                created_ms: now_ms(),
                ready_deadline_ms: 0,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: Some("codex".into()),
                create_operation_id: None,
                binding_id: None,
            },
        }]);
        assert!(registered_available_peer_for_admission(&server, "master").is_none());
        assert_eq!(
            idle_managed_subagent_for_admission(&server, "master")
                .map(|(id, _, _)| id)
                .as_deref(),
            Some("existing-child")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_uses_the_native_appserver_answer_for_a_registered_peer() {
        // A tmux pane is an address, never a liveness credential, so a tmux
        // registration can no longer prove presence. A natively registered
        // App Server peer whose thread answers is the only admission-eligible
        // peer; the pane is not consulted.
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register_appserver_worker(&server, &root, "not-loaded-peer", "thread-not-loaded-peer");

        assert_eq!(
            registered_available_peer_for_admission(&server, "master")
                .map(|(id, _)| id)
                .as_deref(),
            Some("not-loaded-peer")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn omitted_id_reuses_managed_idle_capacity_and_explicit_existing_id_is_preserved() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        // The managed peer is the dispatch recipient, so it needs a
        // genuinely-observed native runtime; a tmux pane is only an address and
        // cannot be admitted as a live recipient.
        register_appserver_worker(&server, &root, "managed-peer", "thread-managed-peer");
        promote_master(&server);
        server.commit(&[Event::SubagentUpdated {
            subagent: crate::subagent::Record {
                id: "existing-child".into(),
                parent: "master".into(),
                peer: "managed-peer".into(),
                status: "idle".into(),
                thread_id: Some("thread-managed-peer".into()),
                profile: None,
                created_ms: now_ms(),
                ready_deadline_ms: 0,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: Some("codex".into()),
                create_operation_id: None,
                binding_id: None,
            },
        }]);
        let server = Arc::new(server);
        let reused = dispatch(
            &server,
            Req::Subagent {
                worker_id: "master".into(),
                token: "token-master".into(),
                command: crate::subagent::Action::Start {
                    id: None,
                    runtime: Some("codex".into()),
                },
                launch_env: Default::default(),
            },
        );
        assert!(!reused.ok, "tmux cannot create a managed Codex thread");
        assert!(reused
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));

        let existing = crate::subagent::handle_with_env(
            &server,
            "master",
            "token-master",
            crate::subagent::Action::Start {
                id: Some("existing-child".into()),
                runtime: Some("codex".into()),
            },
            Default::default(),
        );
        assert!(!existing.ok);
        assert!(existing
            .error
            .unwrap()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));

        let invalid_id = crate::subagent::handle_with_env(
            &server,
            "master",
            "token-master",
            crate::subagent::Action::Start {
                id: Some("invalid id".into()),
                runtime: Some("codex".into()),
            },
            Default::default(),
        );
        assert!(!invalid_id.ok);
        assert!(invalid_id
            .error
            .unwrap()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));
        let invalid_runtime = crate::subagent::handle_with_env(
            &server,
            "master",
            "token-master",
            crate::subagent::Action::Start {
                id: Some("existing-child".into()),
                runtime: Some("unknown".into()),
            },
            Default::default(),
        );
        assert!(!invalid_runtime.ok);
        assert!(invalid_runtime
            .error
            .unwrap()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_unknown_id_bypasses_registered_and_managed_capacity_reuse() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "idle-peer", "%idle-peer");
        register(&server, "managed-peer", "%managed-peer");
        promote_master(&server);
        server.commit(&[Event::SubagentUpdated {
            subagent: crate::subagent::Record {
                id: "existing-child".into(),
                parent: "master".into(),
                peer: "managed-peer".into(),
                status: "idle".into(),
                thread_id: Some("thread-managed-peer".into()),
                profile: None,
                created_ms: now_ms(),
                ready_deadline_ms: 0,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: Some("codex".into()),
                create_operation_id: None,
                binding_id: None,
            },
        }]);

        let admission = scheduler_admit_subagent_start(
            &server,
            "master",
            "token-master",
            Some("requested-child"),
            Some("codex"),
        )
        .unwrap_err();
        assert!(admission
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_subagent_admission_honors_configured_cap() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        promote_master(&server);
        server.config.subagent.max_concurrent = 2;
        server.commit(&[
            Event::SubagentUpdated {
                subagent: crate::subagent::Record {
                    id: "child-a".into(),
                    parent: "master".into(),
                    peer: "managed-a".into(),
                    status: "idle".into(),
                    thread_id: Some("thread-managed-a".into()),
                    profile: None,
                    created_ms: now_ms(),
                    ready_deadline_ms: now_ms() + 10_000,
                    last_message: None,
                    error: None,
                    probe_failures: vec![],
                    runtime: Some("codex".into()),
                    create_operation_id: None,
                    binding_id: None,
                },
            },
            Event::Registered {
                worker: WorkerRec {
                    id: "managed-a".into(),
                    token: "token-managed-a".into(),
                    cwd: root.display().to_string(),
                    registered_ms: now_ms(),
                    transport: Some(test_appserver_transport("thread-managed-a")),
                },
            },
        ]);

        let unsupported =
            scheduler_admit_subagent_start(&server, "master", "token-master", None, Some("codex"))
                .unwrap_err();
        assert!(unsupported
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_subagent_cap_counts_children_not_ordinary_peers() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "ordinary-peer", "%ordinary-peer");
        promote_master(&server);
        server.config.subagent.max_concurrent = 2;
        server.commit(&[
            Event::SubagentUpdated {
                subagent: crate::subagent::Record {
                    id: "child-a".into(),
                    parent: "master".into(),
                    peer: "managed-a".into(),
                    status: "idle".into(),
                    thread_id: Some("thread-managed-a".into()),
                    profile: None,
                    created_ms: now_ms(),
                    ready_deadline_ms: now_ms() + 10_000,
                    last_message: None,
                    error: None,
                    probe_failures: vec![],
                    runtime: Some("codex".into()),
                    create_operation_id: None,
                    binding_id: None,
                },
            },
            Event::Registered {
                worker: WorkerRec {
                    id: "managed-a".into(),
                    token: "token-managed-a".into(),
                    cwd: root.display().to_string(),
                    registered_ms: now_ms(),
                    transport: Some(test_appserver_transport("thread-managed-a")),
                },
            },
        ]);
        assert_eq!(
            live_managed_subagent_count(&server, "master"),
            1,
            "live AppServer child bindings count as registered peers"
        );
        let unsupported =
            scheduler_admit_subagent_start(&server, "master", "token-master", None, Some("codex"))
                .unwrap_err();
        assert!(unsupported
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn master_status_hides_closed_and_lost_idle_capacity() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "closed-peer", "%closed-peer");
        register(&server, "lost-peer", "%lost-peer");
        promote_master(&server);
        server.commit(&[
            Event::SubagentUpdated {
                subagent: crate::subagent::Record {
                    id: "closed-child".into(),
                    parent: "master".into(),
                    peer: "closed-peer".into(),
                    status: "idle".into(),
                    thread_id: Some("thread-closed-peer".into()),
                    profile: None,
                    created_ms: now_ms(),
                    ready_deadline_ms: now_ms() + 10_000,
                    last_message: None,
                    error: None,
                    probe_failures: vec![],
                    runtime: Some("codex".into()),
                    create_operation_id: None,
                    binding_id: None,
                },
            },
            Event::MasterWakeSignal {
                signal: MasterWakeSignal::SubagentStatus {
                    subagent_id: "closed-child".into(),
                },
                at_ms: now_ms(),
            },
            Event::MasterWakeSignal {
                signal: MasterWakeSignal::WorkerIdle {
                    worker_id: "lost-peer".into(),
                },
                at_ms: now_ms(),
            },
        ]);
        assert!(server
            .state
            .lock()
            .unwrap()
            .master_wake
            .idle_workers
            .contains(&"subagent:closed-child".into()));
        server.commit(&[
            Event::SubagentUpdated {
                subagent: crate::subagent::Record {
                    id: "closed-child".into(),
                    parent: "master".into(),
                    peer: "closed-peer".into(),
                    status: "closed".into(),
                    thread_id: Some("thread-closed-peer".into()),
                    profile: None,
                    created_ms: now_ms(),
                    ready_deadline_ms: now_ms() + 10_000,
                    last_message: None,
                    error: None,
                    probe_failures: vec![],
                    runtime: Some("codex".into()),
                    create_operation_id: None,
                    binding_id: None,
                },
            },
            Event::WorkerClosed {
                worker_id: "closed-peer".into(),
                closed_by: "master".into(),
                reason: "done".into(),
                snapshot_captured_ms: Some(now_ms()),
                at_ms: now_ms(),
            },
        ]);
        server.appserver_candidate_check = Arc::new(|candidate| {
            if candidate.thread_id == "thread-lost-peer" {
                Err(crate::client::adapters::AdapterError::RouteUnavailable {
                    detail: "lost peer route".into(),
                }
                .to_string())
            } else {
                Ok(test_appserver_transport(&candidate.thread_id))
            }
        });

        let status = handle_master_status(&server);
        assert!(status.ok, "{status:?}");
        let idle_workers = status.data["master"]["master_wake"]["idle_workers"]
            .as_array()
            .unwrap();
        assert!(
            !idle_workers
                .iter()
                .any(|id| id.as_str() == Some("subagent:closed-child")),
            "closed child must not remain idle capacity"
        );
        assert!(
            !idle_workers
                .iter()
                .any(|id| id.as_str() == Some("lost-peer")),
            "lost peer must not remain idle capacity"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn idle_capacity_projection_filters_execution_and_observation() {
        let (mut server, root) = test_server();
        register_appserver_worker(&server, &root, "master", "thread-master");
        promote_master(&server);
        for worker_id in [
            "busy-peer",
            "accepted-peer",
            "rework-peer",
            "idle-peer",
            "blocked-peer",
            "waiting-peer",
            "working-observation-peer",
            "missing-peer",
        ] {
            seed_live_observed_idle(&server, &root, worker_id);
        }
        server.commit(&[
            Event::TaskCreated {
                task: idle_projection_task("busy", "busy-peer", "working"),
            },
            Event::TaskCreated {
                task: idle_projection_task("accepted", "accepted-peer", "accepted"),
            },
            Event::TaskCreated {
                task: idle_projection_task("rework", "rework-peer", "rework"),
            },
            Event::TaskCreated {
                task: idle_projection_task("blocked", "blocked-peer", "blocked"),
            },
            Event::TaskCreated {
                task: idle_projection_task("waiting", "waiting-peer", "waiting"),
            },
            Event::KeepaliveUpdated {
                worker_id: "working-observation-peer".into(),
                record: crate::server::keepalive::Record {
                    observed: "working".into(),
                    ..Default::default()
                },
            },
        ]);
        let mut accumulator = server.state.lock().unwrap().master_wake.clone();
        accumulator.idle_workers.push("unresponsive-peer".into());
        accumulator
            .newly_idle_workers
            .push("unresponsive-peer".into());
        accumulator
            .unresponsive_workers
            .push("unresponsive-peer".into());
        server.commit(&[Event::MasterWakeUpdated { accumulator }]);
        server.appserver_thread_status = Arc::new(|_, thread_id| {
            if thread_id == "thread-missing-peer" {
                Err("APP_SERVER_THREAD_MISSING: fixture".into())
            } else {
                Ok(serde_json::json!({
                    "thread": {
                        "id": thread_id,
                        "status": {"type": "idle"},
                        "canAcceptDirectInput": true
                    }
                }))
            }
        });

        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let board = handle_board_show(&server, Some("master"));
        let context = context_projection(&server, "master".into(), "token-master".into());
        assert!(board.ok, "{board:?}");
        assert!(context.ok, "{context:?}");
        let after = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        assert_eq!(before, after, "read projections must not mutate state");

        for wake in [
            &board.data["master"]["master_wake"],
            &context.data["master_wake"],
            &context.data["master"]["master_wake"],
        ] {
            let idle = idle_keys(wake);
            for excluded in [
                "busy-peer",
                "accepted-peer",
                "rework-peer",
                "working-observation-peer",
                "missing-peer",
                "unresponsive-peer",
            ] {
                assert!(
                    !idle.iter().any(|id| id == excluded),
                    "{excluded} must not be projected as idle capacity: {wake}"
                );
            }
            for retained in ["idle-peer", "blocked-peer", "waiting-peer"] {
                assert!(
                    idle.iter().any(|id| id == retained),
                    "{retained} must remain idle capacity: {wake}"
                );
            }
            assert!(
                wake["newly_idle_workers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|id| {
                        !matches!(
                            id.as_str(),
                            Some("busy-peer")
                                | Some("accepted-peer")
                                | Some("rework-peer")
                                | Some("working-observation-peer")
                                | Some("missing-peer")
                        )
                    }),
                "newly idle capacity must use the same filter: {wake}"
            );
            assert!(
                wake["unresponsive_workers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|id| id.as_str() == Some("unresponsive-peer")),
                "unresponsive history must survive pruning: {wake}"
            );
        }
        assert_eq!(
            context.data["master_wake"], context.data["master"]["master_wake"],
            "context top-level and nested master capacity must be identical"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn idle_capacity_projection_filters_subagents_and_invitations() {
        let (server, root) = test_server();
        register_appserver_worker(&server, &root, "master", "thread-master");
        promote_master(&server);
        for worker_id in [
            "invitee",
            "publisher",
            "active-child-peer",
            "free-peer",
        ] {
            seed_live_observed_idle(&server, &root, worker_id);
        }
        server.commit(&[
            Event::TaskCreated {
                task: idle_projection_task("offer", "publisher", "invited"),
            },
            Event::BoardDetailsChanged {
                task_id: "offer".into(),
                details: crate::board::BoardTaskDetails {
                    revision: 1,
                    public_visibility: true,
                    invitation: Some(crate::board::BoardInvitation {
                        peer_id: "invitee".into(),
                        binding_id: "fixture-binding".into(),
                        message_id: "offer-message".into(),
                        created_ms: now_ms(),
                        original_status: None,
                        original_owner: None,
                        automatic_source: None,
                    }),
                    ..crate::board::BoardTaskDetails::default()
                },
            },
            Event::TaskCreated {
                task: idle_projection_task("active-child", "active-child-peer", "working"),
            },
            Event::SubagentUpdated {
                subagent: idle_projection_subagent("active-child", "active-child-peer", "idle"),
            },
            Event::MasterWakeSignal {
                signal: MasterWakeSignal::SubagentStatus {
                    subagent_id: "active-child".into(),
                },
                at_ms: now_ms(),
            },
            Event::SubagentUpdated {
                subagent: idle_projection_subagent("closed-child", "invitee", "closed"),
            },
            Event::MasterWakeSignal {
                signal: MasterWakeSignal::SubagentStatus {
                    subagent_id: "closed-child".into(),
                },
                at_ms: now_ms(),
            },
            Event::SubagentUpdated {
                subagent: idle_projection_subagent("lost-child", "lost-peer", "idle"),
            },
            Event::MasterWakeSignal {
                signal: MasterWakeSignal::SubagentStatus {
                    subagent_id: "lost-child".into(),
                },
                at_ms: now_ms(),
            },
            Event::SubagentUpdated {
                subagent: idle_projection_subagent("free-child", "free-peer", "idle"),
            },
            Event::MasterWakeSignal {
                signal: MasterWakeSignal::SubagentStatus {
                    subagent_id: "free-child".into(),
                },
                at_ms: now_ms(),
            },
        ]);

        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let board = handle_board_show(&server, Some("master"));
        let master_context =
            context_projection(&server, "master".into(), "token-master".into());
        let peer_context =
            context_projection(&server, "free-peer".into(), "token-free-peer".into());
        assert!(board.ok, "{board:?}");
        assert!(master_context.ok, "{master_context:?}");
        assert!(peer_context.ok, "{peer_context:?}");
        let after = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        assert_eq!(before, after, "read projections must not mutate state");

        for wake in [
            &board.data["master"]["master_wake"],
            &master_context.data["master_wake"],
            &master_context.data["master"]["master_wake"],
        ] {
            let idle = idle_keys(wake);
            assert!(
                !idle.iter().any(|id| id == "invitee"),
                "a valid invitation reserves execution capacity: {wake}"
            );
            for excluded in [
                "subagent:active-child",
                "subagent:closed-child",
                "subagent:lost-child",
            ] {
                assert!(
                    !idle.iter().any(|id| id == excluded),
                    "{excluded} must not be idle capacity: {wake}"
                );
            }
            assert!(
                idle.iter().any(|id| id == "subagent:free-child"),
                "a live idle subagent without a reservation must remain: {wake}"
            );
        }
        assert!(
            peer_context.data["master_wake"].is_null(),
            "ordinary peer top-level master capacity must stay null: {peer_context:?}"
        );
        let peer_nested = &peer_context.data["master"]["master_wake"];
        let peer_nested_idle = idle_keys(peer_nested);
        assert!(
            !peer_nested_idle.iter().any(|id| id == "invitee"),
            "ordinary peer nested master capacity must share the filtered invitation view: {peer_context:?}"
        );
        assert!(
            peer_nested_idle.iter().any(|id| id == "subagent:free-child"),
            "ordinary peer nested master capacity must keep a live idle subagent: {peer_context:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_requires_board_invitation_for_ordinary_peer() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register_appserver_worker(&server, &root, "peer", "thread-peer");
        promote_master(&server);
        let server = Arc::new(server);
        let dispatch_request = |subject: &str, body: &str| {
            dispatch(
                &server,
                Req::Subagent {
                    worker_id: "master".into(),
                    token: "token-master".into(),
                    command: crate::subagent::Action::Dispatch {
                        request_id: "req-ordinary-1".into(),
                        subject: subject.into(),
                        body: body.into(),
                        feature_id: Some("feature-1".into()),
                        worktree_path: None,
                        branch: None,
                        base_commit: None,
                        priority: "p1".into(),
                        next_step: None,
                    },
                    launch_env: Default::default(),
                },
            )
        };
        assert_eq!(
            registered_available_peer_for_admission(&server, "master")
                .map(|(id, _)| id)
                .as_deref(),
            Some("peer")
        );
        let first = dispatch_request("Implement feature", "Do the work");
        assert!(!first.ok, "{first:?}");
        assert!(first.error.as_deref().unwrap().contains("BOARD_INVITATION_REQUIRED"));
        assert_eq!(first.data["peer_id"], "peer");
        assert_eq!(first.data["assigned"], false);
        let retry = dispatch_request("different retry text", "no implicit dispatch");
        assert!(!retry.ok, "{retry:?}");
        assert!(retry.error.as_deref().unwrap().contains("BOARD_INVITATION_REQUIRED"));
        let state = server.state.lock().unwrap();
        assert!(state.tasks.is_empty());
        assert!(state.msgs.is_empty());
        assert!(state.scheduler_admissions.is_empty());
        drop(state);
        let replayed = current_state::load(&server.state_path, false).unwrap();
        assert!(replayed.tasks.is_empty());
        assert!(replayed.scheduler_admissions.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_public_peer_refusal_does_not_fall_back_to_private_child() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register_appserver_worker(&server, &root, "idle-peer", "thread-idle-peer");
        register_appserver_worker(&server, &root, "managed-peer", "thread-managed-peer");
        promote_master(&server);
        server.commit(&[Event::SubagentUpdated {
            subagent: crate::subagent::Record {
                id: "managed-child".into(),
                parent: "master".into(),
                peer: "managed-peer".into(),
                status: "idle".into(),
                thread_id: Some("thread-managed".into()),
                profile: None,
                created_ms: now_ms(),
                ready_deadline_ms: now_ms() + 10_000,
                last_message: None,
                error: None,
                probe_failures: vec![],
                runtime: Some("codex".into()),
                create_operation_id: None,
                binding_id: None,
            },
        }]);
        let server = Arc::new(server);
        let request = || {
            dispatch(
                &server,
                Req::Subagent {
                    worker_id: "master".into(),
                    token: "token-master".into(),
                    command: crate::subagent::Action::Dispatch {
                        request_id: "req-peer-before-managed".into(),
                        subject: "Peer first".into(),
                        body: "Use the ordinary peer before managed child".into(),
                        feature_id: Some("c5eb401".into()),
                        worktree_path: None,
                        branch: None,
                        base_commit: None,
                        priority: "p0".into(),
                        next_step: None,
                    },
                    launch_env: Default::default(),
                },
            )
        };

        let first = request();
        assert!(!first.ok, "{first:?}");
        assert!(first.error.as_deref().unwrap().contains("BOARD_INVITATION_REQUIRED"));
        assert_eq!(first.data["peer_id"], "idle-peer");
        assert_eq!(first.data["assigned"], false);
        let retry = request();
        assert!(!retry.ok, "{retry:?}");
        assert_eq!(retry.data["peer_id"], "idle-peer");
        let state = server.state.lock().unwrap();
        assert!(state.tasks.is_empty());
        assert!(state.msgs.is_empty());
        assert!(state.scheduler_admissions.is_empty());
        assert_eq!(state.subagents["managed-child"].status, "idle");
        assert_eq!(state.subagents["managed-child"].last_message, None);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_notification_rejection_keeps_retryable_reservation() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        promote_master(&server);
        let reject_once = Arc::new(AtomicBool::new(true));
        let reject_once_for_sink = reject_once.clone();
        server.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
            if reject_once_for_sink.swap(false, Ordering::SeqCst) {
                Err(
                    "ADAPTER_ROUTE_UNAVAILABLE: recipient thread is not loaded by the App Server"
                        .into(),
                )
            } else {
                Ok(json!({"accepted": true}))
            }
        });
        let server = Arc::new(server);
        let request = || {
            dispatch(
                &server,
                Req::Subagent {
                    worker_id: "master".into(),
                    token: "token-master".into(),
                    command: crate::subagent::Action::Dispatch {
                        request_id: "req-notify-rejected-1".into(),
                        subject: "Rejected notification".into(),
                        body: "Must not be assigned".into(),
                        feature_id: None,
                        worktree_path: None,
                        branch: None,
                        base_commit: None,
                        priority: "p0".into(),
                        next_step: None,
                    },
                    launch_env: Default::default(),
                },
            )
        };

        let first = request();
        assert!(!first.ok, "{first:?}");
        assert_eq!(first.data["reservation"], true);
        assert_eq!(first.data["admission"]["status"], "pending");
        assert!(first
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("notification was not accepted"));

        let accepted = dispatch(
            &server,
            Req::TaskAccept {
                worker_id: "peer".into(),
                token: "token-peer".into(),
                task_id: first.data["task_id"].as_str().unwrap().into(),
            },
        );
        assert!(!accepted.ok, "{accepted:?}");
        assert!(accepted
            .error
            .unwrap_or_default()
            .contains("provenance is missing"));

        let retry = request();
        assert!(retry.ok, "{retry:?}");
        assert_eq!(retry.data["recovered"], true);
        assert_eq!(retry.data["task_id"], first.data["task_id"]);
        assert_eq!(retry.data["message_id"], first.data["message_id"]);
        assert_eq!(retry.data["admission"]["status"], "succeeded");

        let state = server.state.lock().unwrap();
        assert_eq!(
            state.scheduler_admissions["req-notify-rejected-1"].status,
            "succeeded"
        );
        assert_eq!(
            state.scheduler_admissions["req-notify-rejected-1"]
                .error
                .as_deref(),
            None
        );
        assert_eq!(
            state.tasks["task-scheduler-req-notify-rejected-1"].status,
            "assigned"
        );
        assert_eq!(
            state.msgs["scheduler-req-notify-rejected-1"].wake_attempt_count,
            2
        );
        drop(state);

        // Notification admission is not task acceptance or message consumption.
        // The private child keeps its existing private readiness protocol.
        let replayed = current_state::load(&server.state_path, false).unwrap();
        assert_eq!(replayed.scheduler_admissions["req-notify-rejected-1"].status, "succeeded");
        let task = &replayed.tasks["task-scheduler-req-notify-rejected-1"];
        assert_eq!(task.status, "assigned");
        let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
        assert!(board_task_view(&replayed, &control, task).unwrap().is_none(), "private assignment must stay off the public board");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_unknown_notification_outcome_is_not_resent() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        promote_master(&server);
        let calls = Arc::new(AtomicU64::new(0));
        let calls_for_sink = calls.clone();
        server.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
            calls_for_sink.fetch_add(1, Ordering::SeqCst);
            Err("ADAPTER_TIMEOUT: turn/start timed out".into())
        });
        let server = Arc::new(server);
        let request = || {
            dispatch(
                &server,
                Req::Subagent {
                    worker_id: "master".into(),
                    token: "token-master".into(),
                    command: crate::subagent::Action::Dispatch {
                        request_id: "req-notify-unknown-1".into(),
                        subject: "Unknown notification".into(),
                        body: "Must not be resent".into(),
                        feature_id: None,
                        worktree_path: None,
                        branch: None,
                        base_commit: None,
                        priority: "p0".into(),
                        next_step: None,
                    },
                    launch_env: Default::default(),
                },
            )
        };

        let first = request();
        assert!(!first.ok, "{first:?}");
        assert_eq!(first.data["admission"]["status"], "pending");
        let message_id = first.data["message_id"].as_str().unwrap().to_owned();
        {
            let state = server.state.lock().unwrap();
            assert!(!state.notification_delivery_failures[&message_id].retryable);
        }

        let retry = request();
        assert!(!retry.ok, "{retry:?}");
        assert_eq!(retry.data["admission"]["status"], "pending");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_reuses_managed_child_and_deduplicates_request() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register_appserver_worker(&server, &root, "managed-peer", "thread-managed-peer");
        promote_master(&server);
        server.commit(&[Event::SubagentUpdated {
            subagent: crate::subagent::Record {
                id: "managed-child".into(),
                parent: "master".into(),
                peer: "managed-peer".into(),
                status: "idle".into(),
                thread_id: Some("thread-managed-peer".into()),
                profile: None,
                created_ms: now_ms(),
                ready_deadline_ms: 0,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: Some("codex".into()),
                create_operation_id: None,
                binding_id: None,
            },
        }]);
        let server = Arc::new(server);
        let request = || {
            dispatch(
                &server,
                Req::Subagent {
                    worker_id: "master".into(),
                    token: "token-master".into(),
                    command: crate::subagent::Action::Dispatch {
                        request_id: "req-managed-1".into(),
                        subject: "Managed task".into(),
                        body: "Use existing child".into(),
                        feature_id: None,
                        worktree_path: None,
                        branch: None,
                        base_commit: None,
                        priority: "p2".into(),
                        next_step: None,
                    },
                    launch_env: Default::default(),
                },
            )
        };
        let first = request();
        assert!(first.ok, "{first:?}");
        assert_eq!(first.data["decision"], "reuse-idle-managed-subagent");
        assert_eq!(first.data["managed_subagent_id"], "managed-child");
        let second = request();
        assert!(second.ok, "{second:?}");
        assert_eq!(second.data["decision"], "deduplicated");
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.msgs.len(), 1);
        assert_eq!(state.subagents["managed-child"].status, "assigned");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_persisted_failed_reservation_is_stable_on_retry() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        promote_master(&server);
        seed_private_reservation(&server, "req-failed-reservation-1", "failed");
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let server = Arc::new(server);
        let request = |body: &str| {
            dispatch(
                &server,
                Req::Subagent {
                    worker_id: "master".into(),
                    token: "token-master".into(),
                    command: crate::subagent::Action::Dispatch {
                        request_id: "req-failed-reservation-1".into(),
                        subject: "Changed subject is ignored".into(),
                        body: body.into(),
                        feature_id: None,
                        worktree_path: None,
                        branch: None,
                        base_commit: None,
                        priority: "p2".into(),
                        next_step: None,
                    },
                    launch_env: Default::default(),
                },
            )
        };
        let first = request("first body");
        assert!(!first.ok, "{first:?}");
        let first_error = first.error.clone().unwrap();
        assert_eq!(first_error, "stored reservation failed");
        assert_eq!(first.data["reservation"], true);
        assert_eq!(first.data["message_id"], "scheduler-req-failed-reservation-1");
        assert_eq!(first.data["task_id"], "task-scheduler-req-failed-reservation-1");
        let second = request("retry body is ignored");
        assert!(!second.ok, "{second:?}");
        assert_eq!(second.error.as_deref(), Some(first_error.as_str()));
        assert_eq!(second.data["reservation"], true);
        assert_eq!(second.data["message_id"], first.data["message_id"]);
        assert_eq!(second.data["task_id"], first.data["task_id"]);
        assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), before);
        assert_eq!(serde_json::to_value(current_state::load(&server.state_path, false).unwrap()).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_child_working_requires_its_succeeded_reservation() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        promote_master(&server);
        let working = || crate::subagent::handle_with_env(
            &server, "peer", "token-peer",
            crate::subagent::Action::Working { id: "managed-peer".into() }, Default::default());
        for status in ["pending", "failed"] {
            seed_private_reservation(&server, "req-working-1", status);
            let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
            let denied = working();
            assert!(!denied.ok, "{denied:?}");
            assert!(denied.error.unwrap().contains(&format!("scheduler assignment admission is {status}")));
            assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), before);
        }
        seed_private_reservation(&server, "req-working-1", "succeeded");
        let mut wrong_child = server.state.lock().unwrap().scheduler_admissions["req-working-1"].clone();
        wrong_child.managed_subagent_id = Some("different-child".into());
        server.commit(&[Event::SchedulerAdmission { admission: wrong_child }]);
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let denied = working();
        assert!(!denied.ok, "{denied:?}");
        assert!(denied.error.unwrap().contains("managed subagent does not own scheduler task"));
        assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), before);
        seed_private_reservation(&server, "req-working-1", "succeeded");
        let before_message = serde_json::to_value(&server.state.lock().unwrap().msgs["scheduler-req-working-1"]).unwrap();
        let accepted = working();
        assert!(accepted.ok, "{accepted:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["managed-peer"].status, "working");
        assert_eq!(state.tasks["task-scheduler-req-working-1"].status, "working");
        assert_eq!(serde_json::to_value(&state.msgs["scheduler-req-working-1"]).unwrap(), before_message);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_reservation_is_hidden_from_poll_inbox_and_notification() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        promote_master(&server);
        server.config.notifications.enabled = true;
        seed_private_reservation(&server, "req-hidden-failed-1", "failed");
        let attempts = Arc::new(AtomicUsize::new(0));
        let sink_attempts = Arc::clone(&attempts);
        server.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
            sink_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"accepted": true}))
        });
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let attempt = attempt_notification_detailed_with_at(&server,
            "scheduler-req-hidden-failed-1", "sub-default-direct-message-peer", now_ms());
        assert!(matches!(attempt, NotificationAttempt::NotAttempted(ref error) if error == "scheduler admission is not deliverable"));
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        let server = Arc::new(server);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let poll_result = runtime.block_on(handle_poll_async(Arc::clone(&server), "peer".into(), 100));
        assert!(poll_result.ok, "{poll_result:?}");
        assert_eq!(poll_result.data["count"], 0);
        assert_eq!(poll_result.data["timeout"], true);
        let state = server.state.lock().unwrap();
        assert_eq!(state.inbox_of("peer").len(), 0);
        assert_eq!(serde_json::to_value(&*state).unwrap(), before);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_notification_reconciles_consumption_before_batch_selection() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        promote_master(&server);
        server.config.notifications.enabled = true;
        seed_private_reservation(&server, "req-consumed-before-wake", "pending");
        let calls = Arc::new(AtomicUsize::new(0));
        let sink_calls = calls.clone();
        server.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
            sink_calls.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"accepted":true}))
        });
        let before_task = serde_json::to_value(&server.state.lock().unwrap().tasks["task-scheduler-req-consumed-before-wake"]).unwrap();
        let before_child = serde_json::to_value(&server.state.lock().unwrap().subagents["managed-peer"]).unwrap();
        let mut expected_message = serde_json::to_value(&server.state.lock().unwrap().msgs["scheduler-req-consumed-before-wake"]).unwrap();
        expected_message["state"] = json!("read");
        let context = private_peer_context(&server);
        let server = Arc::new(server);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let poll = runtime.block_on(handle_poll_async_with_context(server.clone(), "peer".into(),
            Some("token-peer".into()), 100, Some(context.clone()), Some("receive-before-wake".into())));
        assert!(poll.ok, "{poll:?}");
        assert_eq!(poll.data["count"], 1);
        assert_eq!(poll.data["messages"][0]["body"], expected_message["body"]);
        let attempt = attempt_scheduler_notification(&server, "req-consumed-before-wake",
            "scheduler-req-consumed-before-wake", "sub-default-direct-message-peer");
        assert_eq!(attempt.unwrap(), SchedulerNotificationAttempt::Consumed);
        let state = server.state.lock().unwrap();
        assert_eq!(state.scheduler_admissions["req-consumed-before-wake"].status, "succeeded");
        assert_eq!(state.receive_receipts["receive-before-wake"].worker_id, "peer");
        assert_eq!(state.receive_receipts["receive-before-wake"].message_ids, vec!["scheduler-req-consumed-before-wake"]);
        assert_eq!(serde_json::to_value(&state.tasks["task-scheduler-req-consumed-before-wake"]).unwrap(), before_task);
        assert_eq!(serde_json::to_value(&state.subagents["managed-peer"]).unwrap(), before_child);
        assert_eq!(serde_json::to_value(&state.msgs["scheduler-req-consumed-before-wake"]).unwrap(), expected_message);
        assert!(!state.notification_delivery_accepted.contains_key("scheduler-req-consumed-before-wake"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(state);
        let second = runtime.block_on(handle_poll_async_with_context(server.clone(), "peer".into(),
            Some("token-peer".into()), 100, Some(context), Some("receive-after-wake".into())));
        assert!(second.ok, "{second:?}");
        assert_eq!(second.data["count"], 0);
        let loaded = current_state::load(&server.state_path, false).unwrap();
        assert_eq!(loaded.scheduler_admissions["req-consumed-before-wake"].status, "succeeded");
        assert_eq!(serde_json::to_value(&loaded.msgs["scheduler-req-consumed-before-wake"]).unwrap(), expected_message);
        let newer_claim = now_ms() + 1;
        server.commit_checked(&[Event::SchedulerAdmissionStatus {
            request_id: "req-consumed-before-wake".into(), status: "notifying".into(), error: None,
            updated_ms: newer_claim,
        }]).unwrap();
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        assert_eq!(finish_scheduler_consumption(&server, "req-consumed-before-wake",
            "scheduler-req-consumed-before-wake", newer_claim - 1).unwrap(), SchedulerNotificationAttempt::InFlight);
        assert_eq!(finish_scheduler_consumption(&server, "req-consumed-before-wake",
            "another-message", newer_claim).unwrap(), SchedulerNotificationAttempt::Rejected);
        assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), before);
        let saved_path = root.join("saved-current-state.json");
        std::fs::rename(&server.state_path, &saved_path).unwrap();
        std::fs::create_dir(&server.state_path).unwrap();
        let error = finish_scheduler_consumption(&server, "req-consumed-before-wake",
            "scheduler-req-consumed-before-wake", newer_claim).unwrap_err();
        assert!(error.error.as_deref().unwrap().starts_with("SCHEDULER_DURABILITY_FAILED:"), "{error:?}");
        assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), before);
        assert_eq!(serde_json::to_value(current_state::load(&saved_path, false).unwrap()).unwrap(), before);
        std::fs::remove_dir(&server.state_path).unwrap();
        std::fs::rename(saved_path, &server.state_path).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_success_wakes_long_poll() {
        use std::future::Future;
        use std::task::{Context, Wake, Waker};
        struct PollWake(tokio::sync::Notify);
        impl Wake for PollWake {
            fn wake(self: Arc<Self>) { self.0.notify_one(); }
            fn wake_by_ref(self: &Arc<Self>) { self.0.notify_one(); }
        }
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        server.config.notifications.enabled = true;
        assert!(server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .contains_key("sub-default-direct-message-peer"));
        promote_master(&server);
        let context = private_peer_context(&server);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (sink_release_tx, sink_release_rx) = std::sync::mpsc::channel();
        let sink_release_rx = Mutex::new(sink_release_rx);
        let calls = Arc::new(AtomicUsize::new(0));
        let sink_calls = calls.clone();
        server.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
            sink_calls.fetch_add(1, Ordering::SeqCst);
            entered_tx.send(()).unwrap();
            sink_release_rx.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(json!({"accepted":true}))
        });
        let server = Arc::new(server);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let (poll_result, dispatch_result) = runtime.block_on(async {
            let mut poll = Box::pin(handle_poll_async_with_context(server.clone(), "peer".into(),
                Some("token-peer".into()), 5_000, Some(context), Some("receive-successful-dispatch".into())));
            let wake = Arc::new(PollWake(tokio::sync::Notify::new()));
            let waker = Waker::from(wake.clone());
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            started_rx.await.unwrap();
            assert!(poll.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
            release_tx.send(()).unwrap();
            blocker.await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), wake.0.notified()).await.unwrap();
            assert!(poll.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
            let dispatch_server = Arc::clone(&server);
            let dispatch_thread = std::thread::spawn(move || {
                dispatch(
                    &dispatch_server,
                    Req::Subagent {
                        worker_id: "master".into(),
                        token: "token-master".into(),
                        command: crate::subagent::Action::Dispatch {
                            request_id: "req-long-poll-success-1".into(),
                            subject: "Successful long poll task".into(),
                            body: "Must wake recv".into(),
                            feature_id: None,
                            worktree_path: None,
                            branch: None,
                            base_commit: None,
                            priority: "p2".into(),
                            next_step: None,
                        },
                        launch_env: Default::default(),
                    },
                )
            });
            tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(5)).unwrap()).await.unwrap();
            let poll_result = tokio::time::timeout(Duration::from_secs(5), poll).await.unwrap();
            sink_release_tx.send(()).unwrap();
            let dispatch_result = tokio::task::spawn_blocking(move || dispatch_thread.join().unwrap()).await.unwrap();
            (poll_result, dispatch_result)
        });
        assert!(poll_result.ok, "{poll_result:?}");
        assert_eq!(poll_result.data["count"], 1);
        assert!(!poll_result.data["timeout"].as_bool().unwrap_or(false));
        assert_eq!(
            poll_result.data["messages"][0]["id"],
            "scheduler-req-long-poll-success-1"
        );
        assert!(dispatch_result.ok, "{dispatch_result:?}");
        assert_eq!(dispatch_result.data["notification"], "sent");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.scheduler_admissions["req-long-poll-success-1"].status,
            "succeeded"
        );
        assert_eq!(
            state.msgs["scheduler-req-long-poll-success-1"].state,
            "read"
        );
        assert_eq!(state.receive_receipts["receive-successful-dispatch"].message_ids, vec!["scheduler-req-long-poll-success-1"]);
        assert_eq!(state.tasks["task-scheduler-req-long-poll-success-1"].status, "assigned");
        assert_eq!(state.subagents["managed-peer"].status, "assigned");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_recovers_pending_reservation_without_duplicates() {
        // Recover a current pending reservation after Poll consumes its message.
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        server.config.notifications.enabled = true;
        promote_master(&server);
        seed_private_reservation(&server, "req-pending-recovery-1", "pending");
        let context = private_peer_context(&server);
        let before_task = serde_json::to_value(&server.state.lock().unwrap().tasks["task-scheduler-req-pending-recovery-1"]).unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let sink_attempts = Arc::clone(&attempts);
        server.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
            sink_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"accepted": true}))
        });
        let server = Arc::new(server);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let poll_result = runtime.block_on(handle_poll_async_with_context(server.clone(), "peer".into(),
            Some("token-peer".into()), 1_000, Some(context), Some("receive-pending-reservation".into())));
        let consumed_message = serde_json::to_value(&server.state.lock().unwrap().msgs["scheduler-req-pending-recovery-1"]).unwrap();
        let recovered = dispatch(
                    &server,
                    Req::Subagent {
                        worker_id: "master".into(),
                        token: "token-master".into(),
                        command: crate::subagent::Action::Dispatch {
                            request_id: "req-pending-recovery-1".into(),
                            subject: "Changed subject is ignored".into(),
                            body: "Changed body is ignored".into(),
                            feature_id: None,
                            worktree_path: None,
                            branch: None,
                            base_commit: None,
                            priority: "p2".into(),
                            next_step: None,
                        },
                        launch_env: Default::default(),
                    },
                );
        assert!(poll_result.ok, "{poll_result:?}");
        assert_eq!(poll_result.data["count"], 1);
        assert!(!poll_result.data["timeout"].as_bool().unwrap_or(false));
        assert_eq!(
            poll_result.data["messages"][0]["id"],
            "scheduler-req-pending-recovery-1"
        );
        assert!(recovered.ok, "{recovered:?}");
        assert_eq!(recovered.data["recovered"], true);
        assert_eq!(recovered.data["decision"], "reuse-idle-managed-subagent");
        assert_eq!(recovered.data["notification"], "already-consumed");
        assert_eq!(recovered.data["message_id"], "scheduler-req-pending-recovery-1");
        assert_eq!(recovered.data["task_id"], "task-scheduler-req-pending-recovery-1");
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs.len(), 1);
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(serde_json::to_value(&state.tasks["task-scheduler-req-pending-recovery-1"]).unwrap(), before_task);
        assert_eq!(serde_json::to_value(&state.msgs["scheduler-req-pending-recovery-1"]).unwrap(), consumed_message);
        assert_eq!(state.subagents["managed-peer"].status, "assigned");
        assert_eq!(state.receive_receipts["receive-pending-reservation"].worker_id, "peer");
        assert_eq!(state.receive_receipts["receive-pending-reservation"].message_ids, vec!["scheduler-req-pending-recovery-1"]);
        assert_eq!(
            state.scheduler_admissions["req-pending-recovery-1"].status,
            "succeeded"
        );
        drop(state);
        let repeated_poll = runtime.block_on(handle_poll_async(Arc::clone(&server), "peer".into(), 100));
        assert!(repeated_poll.ok, "{repeated_poll:?}");
        assert_eq!(repeated_poll.data["count"], 0);
        std::fs::remove_dir_all(root).unwrap();
    }
