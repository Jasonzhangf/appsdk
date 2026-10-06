    use super::*;
    use crate::server::peer_tests::{register, test_appserver_transport, test_server};
    use crate::server::state::MasterWakeSignal;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Condvar, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    // Notification/recovery tests use the retained private-child dispatcher.
    // Public ordinary-peer dispatch is tested as an explicit board-only gate.
    fn register_private_dispatch_peer(server: &Server) {
        register(server, "peer", "%peer");
        server.commit(&[Event::SubagentUpdated { subagent: crate::subagent::Record {
            id: "managed-peer".into(), parent: "master".into(), peer: "peer".into(),
            status: "idle".into(), thread_id: None, profile: None,
            created_ms: now_ms(), ready_deadline_ms: now_ms() + 60_000,
            last_message: None, error: None, probe_failures: vec![], runtime: Some("codex".into()),
        } }]);
    }

    fn promote_master(server: &Server) {
        let response = handle_master_promote(
            server,
            "master".into(),
            "token-master".into(),
            "scheduler test".into(),
        );
        assert!(response.ok, "master promotion failed: {response:?}");
    }

    #[test]
    fn start_admits_registered_idle_peer_before_managed_child_without_duplicates() {
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
        drop(state);
        assert!(
            !std::fs::read_to_string(root.join(".agent-collab/server/events.jsonl"))
                .unwrap_or_default()
                .contains("scheduler_admission")
        );

        let denied = start(None, "wrong-token");
        assert!(!denied.ok);
        assert!(denied.error.unwrap().contains("authentication failed"));
        let state = server.state.lock().unwrap();
        assert!(state.subagents.is_empty());
        assert!(state.tasks.is_empty());
        assert!(state.msgs.is_empty());
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
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        }]);
        assert!(registered_available_peer_for_admission(&server, "master").is_none());
        std::fs::remove_dir_all(root).unwrap();

        let (server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "managed-peer", "%managed-peer");
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
    fn admission_uses_the_appserver_answer_for_a_registered_tmux_peer() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "not-loaded-peer", "%not-loaded-peer");
        // A tmux registration is live only when its explicitly registered
        // AppServer thread answers; the pane alone is not a liveness proof.
        server.appserver_thread_status = Arc::new(|_, thread_id| {
            Ok(serde_json::json!({
                "thread": {"id": thread_id, "status": {"type": "idle"}},
                "thread_state": "idle"
            }))
        });

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
    fn admission_audit_failure_is_explicit_and_does_not_create_child() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "idle-peer", "%idle-peer");
        promote_master(&server);
        let events = root.join(".agent-collab/server/events.jsonl");
        std::fs::create_dir(&events).unwrap();
        let server = Arc::new(server);
        let result = dispatch(
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
        assert!(!result.ok);
        assert!(result
            .error
            .unwrap()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));
        assert!(server.state.lock().unwrap().subagents.is_empty());
        assert!(server.state.lock().unwrap().scheduler_admissions.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn no_capacity_records_explicit_create_admission() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        promote_master(&server);
        let unsupported =
            scheduler_admit_subagent_start(&server, "master", "token-master", None, Some("codex"))
                .unwrap_err();
        assert!(unsupported
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("MANAGED_SUBAGENT_UNSUPPORTED"));
        let audit = std::fs::read_to_string(root.join(".agent-collab/server/events.jsonl"))
            .unwrap_or_default();
        assert!(!audit.contains("create-managed-subagent"));
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
    fn scheduler_dispatch_requires_board_invitation_for_ordinary_peer() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "peer", "%peer");
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
        let replayed = replay(&root).unwrap();
        assert!(replayed.tasks.is_empty());
        assert!(replayed.scheduler_admissions.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_public_peer_refusal_does_not_fall_back_to_private_child() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "idle-peer", "%idle-peer");
        register(&server, "managed-peer", "%managed-peer");
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
        assert!(!root.join(".agent-collab/server/events.jsonl").exists(),
            "rejected ordinary dispatch must not create an admission audit");
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
        let replayed = replay(&root).unwrap();
        assert_eq!(replayed.scheduler_admissions["req-notify-rejected-1"].status, "succeeded");
        let task = &replayed.tasks["task-scheduler-req-notify-rejected-1"];
        assert_eq!(task.status, "assigned");
        assert!(board_task_view(&replayed, task).is_none(), "private assignment must stay off the public board");
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
        register(&server, "managed-peer", "%managed-peer");
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
    fn scheduler_dispatch_audit_failure_is_stable_on_request_retry() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        promote_master(&server);
        let activity_path = root.join(".agent-collab/server/events.jsonl");
        std::fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        std::fs::create_dir(&activity_path).unwrap();
        let server = Arc::new(server);
        let request = |body: &str| {
            dispatch(
                &server,
                Req::Subagent {
                    worker_id: "master".into(),
                    token: "token-master".into(),
                    command: crate::subagent::Action::Dispatch {
                        request_id: "req-audit-failure-1".into(),
                        subject: "Audit failure task".into(),
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
        assert!(first_error.contains("scheduler admission audit failed"));
        assert_eq!(first.data["reservation"], true);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs["scheduler-req-audit-failure-1"].state, "pending");
        assert_eq!(
            state.msgs["scheduler-req-audit-failure-1"].wake_attempt_count,
            0
        );
        drop(state);
        *server.state.lock().unwrap() = replay(&root).unwrap();
        let second = request("retry body is ignored");
        assert!(!second.ok, "{second:?}");
        assert_eq!(second.error.as_deref(), Some(first_error.as_str()));
        assert_eq!(second.data["reservation"], true);
        assert_eq!(second.data["message_id"], first.data["message_id"]);
        assert_eq!(second.data["task_id"], first.data["task_id"]);
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.msgs.len(), 1);
        assert_eq!(
            state.scheduler_admissions["req-audit-failure-1"].status,
            "failed"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_audit_failure_cannot_be_accepted_by_managed_child() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "managed-peer", "%managed-peer");
        promote_master(&server);
        server.commit(&[Event::SubagentUpdated {
            subagent: crate::subagent::Record {
                id: "managed-child-failed-audit".into(),
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
            },
        }]);
        std::fs::create_dir(root.join(".agent-collab/server/events.jsonl")).unwrap();
        let server = Arc::new(server);
        let dispatch_result = dispatch(
            &server,
            Req::Subagent {
                worker_id: "master".into(),
                token: "token-master".into(),
                command: crate::subagent::Action::Dispatch {
                    request_id: "req-managed-failed-audit-1".into(),
                    subject: "Failed managed task".into(),
                    body: "Must not execute".into(),
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
        assert!(!dispatch_result.ok, "{dispatch_result:?}");
        let working = crate::subagent::handle_with_env(
            &server,
            "managed-peer",
            "token-managed-peer",
            crate::subagent::Action::Working {
                id: "managed-child-failed-audit".into(),
            },
            Default::default(),
        );
        assert!(!working.ok, "{working:?}");
        assert!(working
            .error
            .unwrap_or_default()
            .contains("scheduler assignment admission is failed"));
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.scheduler_admissions["req-managed-failed-audit-1"].status,
            "failed"
        );
        assert_eq!(
            state.subagents["managed-child-failed-audit"].status,
            "assigned"
        );
        assert_eq!(
            state.tasks["task-scheduler-req-managed-failed-audit-1"].status,
            "assigned"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_audit_failure_does_not_wake_long_poll_or_recv() {
        let (server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        promote_master(&server);
        std::fs::create_dir(root.join(".agent-collab/server/events.jsonl")).unwrap();
        let server = Arc::new(server);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (poll_result, dispatch_result) = runtime.block_on(async {
            let poll = handle_poll_async(Arc::clone(&server), "peer".into(), 100);
            let dispatch_server = Arc::clone(&server);
            let dispatch = tokio::task::spawn_blocking(move || {
                std::thread::sleep(Duration::from_millis(10));
                dispatch(
                    &dispatch_server,
                    Req::Subagent {
                        worker_id: "master".into(),
                        token: "token-master".into(),
                        command: crate::subagent::Action::Dispatch {
                            request_id: "req-long-poll-failed-audit-1".into(),
                            subject: "Failed long poll task".into(),
                            body: "Must not be consumed".into(),
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
            let (poll_result, dispatch_result) = tokio::join!(poll, dispatch);
            (poll_result, dispatch_result.unwrap())
        });
        assert!(poll_result.ok, "{poll_result:?}");
        assert_eq!(poll_result.data["count"], 0);
        assert_eq!(poll_result.data["timeout"], true);
        assert!(!dispatch_result.ok, "{dispatch_result:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.msgs["scheduler-req-long-poll-failed-audit-1"].wake_attempt_count,
            0
        );
        assert_eq!(state.inbox_of("peer").len(), 0);
        assert_eq!(
            state.scheduler_admissions["req-long-poll-failed-audit-1"].status,
            "failed"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_success_wakes_long_poll() {
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
        let server = Arc::new(server);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (poll_result, dispatch_result) = runtime.block_on(async {
            let poll = handle_poll_async(Arc::clone(&server), "peer".into(), 5_000);
            let dispatch_server = Arc::clone(&server);
            let dispatch = tokio::task::spawn_blocking(move || {
                std::thread::sleep(Duration::from_millis(10));
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
            let (poll_result, dispatch_result) = tokio::join!(poll, dispatch);
            (poll_result, dispatch_result.unwrap())
        });
        assert!(poll_result.ok, "{poll_result:?}");
        assert_eq!(poll_result.data["count"], 1);
        assert!(!poll_result.data["timeout"].as_bool().unwrap_or(false));
        assert_eq!(
            poll_result.data["messages"][0]["id"],
            "scheduler-req-long-poll-success-1"
        );
        assert!(dispatch_result.ok, "{dispatch_result:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.scheduler_admissions["req-long-poll-success-1"].status,
            "succeeded"
        );
        assert_eq!(
            state.msgs["scheduler-req-long-poll-success-1"].state,
            "read"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_recovers_pending_reservation_without_duplicates() {
        // Simulate a process interruption after the reservation and audit write,
        // but before the durable succeeded status commit.
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "peer", "%peer");
        server.config.notifications.enabled = true;
        promote_master(&server);
        server.commit(&[
            Event::Sent {
                msg: Message {
                    id: "scheduler-req-pending-recovery-1".into(),
                    from: "master".into(),
                    to: "peer".into(),
                    mtype: "notify".into(),
                    subject: Some("Pending recovery".into()),
                    body: "Reuse reservation".into(),
                    in_reply_to: None,
                    created_ms: now_ms(),
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            },
            Event::TaskCreated {
                task: TaskRec {
                    id: "task-scheduler-req-pending-recovery-1".into(),
                    owner: "peer".into(),
                    created_by: "master".into(),
                    feature_id: None,
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p2".into(),
                    status: "assigned".into(),
                    next_step: Some("accept".into()),
                    wait: None,
                    created_ms: now_ms(),
                    updated_ms: now_ms(),
                },
            },
            Event::DeliveryMode {
                msg_id: "scheduler-req-pending-recovery-1".into(),
                mode: "explicit-notification".into(),
                source_thread_id: None,
            },
            Event::WakeBound {
                message_id: "scheduler-req-pending-recovery-1".into(),
                subscription_id: "sub-default-direct-message-peer".into(),
            },
            Event::SchedulerAdmission {
                admission: crate::server::state::SchedulerAdmissionRecord {
                    request_id: "req-pending-recovery-1".into(),
                    decision: "use-registered-peer".into(),
                    worker_id: "peer".into(),
                    managed_subagent_id: None,
                    message_id: "scheduler-req-pending-recovery-1".into(),
                    task_id: "task-scheduler-req-pending-recovery-1".into(),
                    status: "pending".into(),
                    error: None,
                    created_ms: now_ms(),
                    updated_ms: now_ms(),
                },
            },
        ]);
        record_scheduler_admission(
            &server,
            json!({
                "request_id": "req-pending-recovery-1",
                "decision": "use-registered-peer",
                "worker_id": "peer",
                "message_id": "scheduler-req-pending-recovery-1",
                "task_id": "task-scheduler-req-pending-recovery-1",
                "status": "pending",
            }),
        )
        .unwrap();
        let audit_path = root.join(".agent-collab/server/events.jsonl");
        let original_mode = std::fs::metadata(&audit_path).unwrap().permissions().mode();
        let mut read_only = std::fs::metadata(&audit_path).unwrap().permissions();
        read_only.set_mode(original_mode & !0o222);
        std::fs::set_permissions(&audit_path, read_only).unwrap();
        let server = Arc::new(server);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (poll_result, recovered) = runtime.block_on(async {
            let poll = handle_poll_async(Arc::clone(&server), "peer".into(), 1_000);
            let recovery_server = Arc::clone(&server);
            let recovery = tokio::task::spawn_blocking(move || {
                dispatch(
                    &recovery_server,
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
                )
            });
            let (poll_result, recovered) = tokio::join!(poll, recovery);
            (poll_result, recovered.unwrap())
        });
        let mut restored = std::fs::metadata(&audit_path).unwrap().permissions();
        restored.set_mode(original_mode);
        std::fs::set_permissions(&audit_path, restored).unwrap();
        assert!(poll_result.ok, "{poll_result:?}");
        assert_eq!(poll_result.data["count"], 1);
        assert!(!poll_result.data["timeout"].as_bool().unwrap_or(false));
        assert_eq!(
            poll_result.data["messages"][0]["id"],
            "scheduler-req-pending-recovery-1"
        );
        assert!(recovered.ok, "{recovered:?}");
        assert_eq!(recovered.data["recovered"], true);
        assert_eq!(recovered.data["decision"], "use-registered-peer");
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs.len(), 1);
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(
            state.scheduler_admissions["req-pending-recovery-1"].status,
            "succeeded"
        );
        drop(state);
        let audit_count = std::fs::read_to_string(root.join(".agent-collab/server/events.jsonl"))
            .unwrap()
            .lines()
            .filter(|line| {
                line.contains("scheduler_admission") && line.contains("req-pending-recovery-1")
            })
            .count();
        assert_eq!(audit_count, 1);
        std::fs::remove_dir_all(root).unwrap();
    }
