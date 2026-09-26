    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn probes_are_bounded_and_require_exact_success() {
        let directory =
            std::env::temp_dir().join(format!("collab-probe-test-{:016x}", rand::random::<u64>()));
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("codex-fixture");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprofile=\noutput=\nwhile [ $# -gt 0 ]; do\n case \"$1\" in\n --profile) shift; profile=$1;;\n --output-last-message) shift; output=$1;;\n esac\n shift\ndone\ncase \"$profile\" in\n good) printf OK > \"$output\";;\n env) if [ -z \"${TMUX+x}\" ] && [ -z \"${TMUX_PANE+x}\" ]; then printf OK > \"$output\"; else printf INHERITED > \"$output\"; fi;;\n wrong) printf NOT_OK > \"$output\";;\n fail) exit 3;;\n slow) exec sleep 2;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let settings = config::Health::default();
        for (name, expected) in [
            ("good", true),
            ("env", true),
            ("wrong", false),
            ("fail", false),
        ] {
            let mut environment: std::collections::BTreeMap<_, _> = std::env::vars().collect();
            environment.insert("TMUX".into(), "/tmp/legacy-tmux".into());
            environment.insert("TMUX_PANE".into(), "%42".into());
            let result = probe_with(
                &executable,
                "codex",
                &config::Profile {
                    codex_profile: name.into(),
                    model: None,
                },
                &settings,
                &environment,
            );
            assert_eq!(result.is_ok(), expected, "profile={name} result={result:?}");
        }
        let mut environment: std::collections::BTreeMap<_, _> = std::env::vars().collect();
        environment.insert("TMUX".into(), "/tmp/legacy-tmux".into());
        environment.insert("TMUX_PANE".into(), "%42".into());
        let start = Instant::now();
        let result = probe_with(
            &executable,
            "codex",
            &config::Profile {
                codex_profile: "slow".into(),
                model: None,
            },
            &config::Health {
                timeout_seconds: 1,
                ..Default::default()
            },
            &environment,
        );
        assert_eq!(result.unwrap_err().to_string(), "probe timed out");
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn cursor_runtime_is_rejected() {
        let mcp = std::path::Path::new("/tmp/collab-mcp");
        let error = launch_args(
            "cursor",
            &config::Profile {
                codex_profile: "oauth".into(),
                model: None,
            },
            std::path::Path::new("/tmp/project"),
            "hello",
            mcp,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("must be codex"), "{error}");
        let (exe, args) = launch_args(
            "codex",
            &config::Profile {
                codex_profile: "oauth".into(),
                model: None,
            },
            std::path::Path::new("/tmp/project"),
            "hello",
            mcp,
        )
        .unwrap();
        assert_eq!(exe, "codex");
        assert_eq!(args[..3], ["--profile", "oauth", "--approve-for-me"]);
        assert!(args.contains(&"--approve-for-me".to_string()));
        assert!(!args.contains(&"dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(!args
            .iter()
            .any(|a| a == "danger-full-access" || a == "--ask-for-approval"));
        assert!(args
            .iter()
            .any(|a| a.contains("mcp_servers.appsdk-subagent")));
        assert!(args
            .iter()
            .any(|a| a.contains("collab_ack") && a.contains("approve")));
        assert!(args.last().unwrap().contains("collab CLI"));
        let prompt = child_prompt(
            &Record {
                id: "child-1".into(),
                parent: "parent-1".into(),
                peer: "peer-1".into(),
                status: "starting".into(),
                thread_id: None,
                profile: None,
                created_ms: 0,
                ready_deadline_ms: 0,
                last_message: None,
                error: None,
                probe_failures: vec![],
                runtime: None,
            },
            &serde_json::json!({
                "role": "managed-subagent",
                "role_task": "Execute the assigned independent task and return evidence to parent/master.",
                "responsibilities": [
                    "Stay inside the assigned task, worktree, file scope, delivery conditions, and tests."
                ],
                "authority": {
                    "managed_subagent": true,
                    "must_obey_master": true,
                    "may_decline_master_invite": false
                },
                "derivation": {
                    "kind": "managed-subagent",
                    "parent": "parent-1"
                },
                "blocked_boundary": "Report a concrete root cause and proposed fix.",
                "completion_action": "Return completed evidence.",
                "next_action": "Continue the assigned task.",
                "notification_rule": "Reading or ACK is never task progress."
            }),
        )
        .unwrap();
        assert!(prompt.contains("collab ack <message-id>"));
        assert!(prompt.contains("already registered"));
        assert!(prompt.contains("Active role contract (from the registration receipt):"));
        assert!(prompt.contains("Role: managed-subagent"));
        assert!(prompt.contains("\"managed_subagent\":true"));
        assert!(prompt.contains("\"must_obey_master\":true"));
        assert!(prompt.contains("\"may_decline_master_invite\":false"));
        assert!(prompt.contains("Derivation"));
        assert!(prompt.contains("parent-1"));
        assert!(!prompt.contains("collab init"));
        assert!(!prompt.contains("worker recover"));
        assert!(prompt.contains("shared Collab MCP"));
        assert!(prompt.contains("collab CLI in this cwd is also valid"));
        assert!(!prompt.contains("NOT sandboxed shell"));
    }

    #[test]
    fn child_appserver_candidate_binds_child_session_and_thread() {
        let parent_transport = crate::proto::SelectedTransport {
            kind: crate::proto::TransportKind::AppServer,
            endpoint: Some("unix:///tmp/collab-parent.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: None,
            thread_id: Some("01a0c48b-parent-thread".into()),
            tmux_endpoint: None,
            capabilities: vec![],
            self_check: "parent verified".into(),
        };
        let child_thread = crate::identity::NativeThreadId::new("01a0c7e7-child-thread").unwrap();
        let root = std::path::Path::new("/tmp/collab-child-project");
        let child_status = serde_json::json!({
            "thread": {
                "id": "01a0c7e7-child-thread",
                "sessionId": "01a0c7e7-child-session"
            }
        });

        let child_session_id = child_session_id_from_thread_status(&child_status).unwrap();
        let candidate = child_appserver_candidate_from_session(
            &parent_transport,
            root,
            &child_session_id,
            &child_thread,
        )
        .unwrap();

        assert_eq!(candidate.endpoint, "unix:///tmp/collab-parent.sock");
        assert_eq!(candidate.namespace, "codex_tui");
        assert_eq!(candidate.thread_id, "01a0c7e7-child-thread");
        assert_eq!(candidate.session_id, "01a0c7e7-child-session");
        assert_ne!(
            candidate.session_id, "01a0c48b-parent-session",
            "child registration must not self-check against the parent session"
        );
    }

    #[test]
    fn missing_rollout_notification_failure_preserves_route_binding() {
        let missing_rollout = crate::client::adapters::AdapterError::Unknown {
            operation: "rpc",
            detail: "no rollout found for thread id 01a0c80e-child-thread".into(),
        };
        assert_eq!(
            child_notification_failure_action(&missing_rollout),
            ChildNotificationFailureAction::PreserveRouteBinding
        );

        let other_rpc_failure = crate::client::adapters::AdapterError::Unknown {
            operation: "rpc",
            detail: "thread not found: 01a0c80e-child-thread".into(),
        };
        assert_eq!(
            child_notification_failure_action(&other_rpc_failure),
            ChildNotificationFailureAction::RetireRouteBinding
        );

        let writer_conflict = crate::client::adapters::AdapterError::ThreadWriterConflict {
            detail: "thread owned by another writer".into(),
        };
        assert_eq!(
            child_notification_failure_action(&writer_conflict),
            ChildNotificationFailureAction::RetireRouteBinding
        );
    }

    fn register_child_route(
        server: &Server,
        root: &std::path::Path,
        child_id: &str,
        thread_id: &str,
    ) -> (
        AppServerId,
        crate::identity::SessionId,
        crate::identity::NativeThreadId,
        crate::scope::RouteScope,
        crate::identity::BindingId,
    ) {
        let app_scope = AppServerId::new("tui-default").unwrap();
        let response = crate::server::peer_tests::register(server, child_id, thread_id);
        assert!(response.ok, "{response:?}");
        crate::server::commit_current_thread_route_for_runtime(
            server,
            server,
            child_id,
            &root.display().to_string(),
            Some(&app_scope),
        )
        .unwrap();
        let session_id = crate::identity::SessionId::new(format!("session-{thread_id}")).unwrap();
        let native_thread_id = crate::identity::NativeThreadId::new(thread_id).unwrap();
        let route_scope = crate::scope::RouteScope {
            app_scope_id: app_scope.clone(),
            project_scope_id: crate::server::GlobalState::canonical_project_scope(root).unwrap(),
        };
        let binding_id = crate::identity::BindingId::new(format!("binding-{child_id}")).unwrap();
        (
            app_scope,
            session_id,
            native_thread_id,
            route_scope,
            binding_id,
        )
    }

    #[test]
    fn close_reports_named_terminal_outcome_when_archive_is_impossible() {
        let (mut server, root) = crate::server::peer_tests::test_server();
        let registered = crate::server::peer_tests::register(&server, "parent", "thread-parent");
        assert!(registered.ok, "{registered:?}");
        let child_id = "child-archive-unavailable";
        let (_app_scope, session_id, native_thread_id, route_scope, binding_id) =
            register_child_route(&server, &root, child_id, "thread-child-archive-unavailable");
        server.commit(&[Event::SubagentUpdated {
            subagent: Record {
                id: "subagent-archive-unavailable".into(),
                parent: "parent".into(),
                peer: child_id.into(),
                status: "idle".into(),
                thread_id: Some(native_thread_id.to_string()),
                profile: None,
                created_ms: 0,
                ready_deadline_ms: 0,
                last_message: None,
                error: None,
                probe_failures: vec![],
                runtime: Some("codex".into()),
            },
        }]);
        server.commit(&[Event::SubagentSnapshotCaptured {
            subagent_id: "subagent-archive-unavailable".into(),
            thread_id: native_thread_id.to_string(),
            captured_ms: now_ms(),
        }]);
        server.appserver_thread_archive = std::sync::Arc::new(|_, _| {
            panic!("tmux close must not call the retired AppServer archive adapter")
        });
        let server = std::sync::Arc::new(server);

        let response = crate::subagent::handle_with_env(
            &server,
            "parent",
            "token-parent",
            Action::Close {
                id: "subagent-archive-unavailable".into(),
            },
            std::collections::BTreeMap::new(),
        );

        assert!(response.ok, "{response:?}");
        assert_eq!(
            response.data["close_outcome"], "closed_record_only",
            "{response:?}"
        );
        assert!(response.data["subagent"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("registration and route remain active")));
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.subagents["subagent-archive-unavailable"].status,
            "closed"
        );
        assert!(state
            .global
            .lookup_current_thread_route(&session_id, &native_thread_id)
            .is_some());
        assert_eq!(
            state
                .global
                .lookup_binding_for(&route_scope, &binding_id)
                .and_then(|binding| binding.native_thread_id.clone()),
            Some(native_thread_id.clone())
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn close_reports_archive_failed_when_archiving_returns_an_unclassified_error() {
        let (mut server, root) = crate::server::peer_tests::test_server();
        let registered = crate::server::peer_tests::register(&server, "parent", "thread-parent");
        assert!(registered.ok, "{registered:?}");
        let child_id = "child-archive-failed";
        let (_app_scope, session_id, native_thread_id, route_scope, binding_id) =
            register_child_route(&server, &root, child_id, "thread-child-archive-failed");
        server.commit(&[Event::SubagentUpdated {
            subagent: Record {
                id: "subagent-archive-failed".into(),
                parent: "parent".into(),
                peer: child_id.into(),
                status: "idle".into(),
                thread_id: Some(native_thread_id.to_string()),
                profile: None,
                created_ms: 0,
                ready_deadline_ms: 0,
                last_message: None,
                error: None,
                probe_failures: vec![],
                runtime: Some("codex".into()),
            },
        }]);
        server.commit(&[Event::SubagentSnapshotCaptured {
            subagent_id: "subagent-archive-failed".into(),
            thread_id: native_thread_id.to_string(),
            captured_ms: now_ms(),
        }]);
        server.appserver_thread_archive = std::sync::Arc::new(|_, _| {
            panic!("tmux close must not call the retired AppServer archive adapter")
        });
        let server = std::sync::Arc::new(server);

        let response = crate::subagent::handle_with_env(
            &server,
            "parent",
            "token-parent",
            Action::Close {
                id: "subagent-archive-failed".into(),
            },
            std::collections::BTreeMap::new(),
        );

        assert!(response.ok, "{response:?}");
        assert_eq!(
            response.data["close_outcome"], "closed_record_only",
            "{response:?}"
        );
        assert!(response.data["subagent"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("registration and route remain active")));
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["subagent-archive-failed"].status, "closed");
        assert!(state
            .global
            .lookup_current_thread_route(&session_id, &native_thread_id)
            .is_some());
        assert_eq!(
            state
                .global
                .lookup_binding_for(&route_scope, &binding_id)
                .and_then(|binding| binding.native_thread_id.clone()),
            Some(native_thread_id.clone())
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_rollout_notification_failure_keeps_durable_child_route_binding() {
        let (server, root) = crate::server::peer_tests::test_server();
        let child_id = "child-missing-rollout";
        let (app_scope, session_id, native_thread_id, route_scope, binding_id) =
            register_child_route(&server, &root, child_id, "thread-child-missing-rollout");
        let parent_transport = crate::server::peer_tests::test_appserver_transport("thread-parent");
        let mut record = Record {
            id: "subagent-missing-rollout".into(),
            parent: "parent-1".into(),
            peer: child_id.into(),
            status: "starting".into(),
            thread_id: None,
            profile: None,
            created_ms: 0,
            ready_deadline_ms: 0,
            last_message: None,
            error: None,
            probe_failures: vec![],
            runtime: Some("codex".into()),
        };

        let error = crate::client::adapters::AdapterError::Unknown {
            operation: "rpc",
            detail: format!(
                "no rollout found for thread id {}",
                native_thread_id.as_str()
            ),
        };
        let result = child_notification_failure_result(
            &server,
            &server,
            &mut record,
            &parent_transport,
            &native_thread_id,
            child_id,
            &root.display().to_string(),
            Some(&app_scope),
            error,
        )
        .unwrap_err()
        .to_string();

        assert!(
            result.contains("route preserved; binding preserved"),
            "{result}"
        );
        assert_eq!(record.thread_id.as_deref(), Some(native_thread_id.as_str()));
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .lookup_binding_for(&route_scope, &binding_id)
            .unwrap();
        assert_eq!(
            binding.native_thread_id.as_ref(),
            Some(&native_thread_id),
            "missing rollout must keep the runtime binding thread-backed"
        );
        assert_eq!(
            state
                .global
                .lookup_current_thread_route(&session_id, &native_thread_id),
            Some(binding),
            "missing rollout must not retire the published child route"
        );
        assert!(state.workers.contains_key(child_id));
    }

    #[test]
    fn other_notification_failure_retires_durable_child_route_binding() {
        let (server, root) = crate::server::peer_tests::test_server();
        let child_id = "child-notification-rpc-failure";
        let (app_scope, session_id, native_thread_id, route_scope, binding_id) =
            register_child_route(&server, &root, child_id, "thread-child-rpc-failure");
        let parent_transport = crate::server::peer_tests::test_appserver_transport("thread-parent");
        let mut record = Record {
            id: "subagent-rpc-failure".into(),
            parent: "parent-1".into(),
            peer: child_id.into(),
            status: "starting".into(),
            thread_id: Some(native_thread_id.to_string()),
            profile: None,
            created_ms: 0,
            ready_deadline_ms: 0,
            last_message: None,
            error: None,
            probe_failures: vec![],
            runtime: Some("codex".into()),
        };

        let error = crate::client::adapters::AdapterError::Unknown {
            operation: "rpc",
            detail: "thread not found".into(),
        };
        let result = child_notification_failure_result(
            &server,
            &server,
            &mut record,
            &parent_transport,
            &native_thread_id,
            child_id,
            &root.display().to_string(),
            Some(&app_scope),
            error,
        )
        .unwrap_err()
        .to_string();

        assert!(result.contains("route retired"), "{result}");
        assert!(result.contains("binding retired"), "{result}");
        assert_eq!(record.thread_id, None);
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .lookup_binding_for(&route_scope, &binding_id)
            .unwrap();
        assert_eq!(
            binding.native_thread_id, None,
            "generic notification failures must retire the runtime binding"
        );
        assert!(
            state
                .global
                .lookup_current_thread_route(&session_id, &native_thread_id)
                .is_none(),
            "generic notification failures must retire the published child route"
        );
        assert!(!state.workers.contains_key(child_id));
    }

    #[test]
    fn failed_subagent_record_with_thread_id_is_not_reused() {
        let mut record = Record {
            id: "child-1".into(),
            parent: "parent-1".into(),
            peer: "peer-1".into(),
            status: "failed".into(),
            thread_id: Some("archived-child-thread".into()),
            profile: None,
            created_ms: 0,
            ready_deadline_ms: 0,
            last_message: None,
            error: Some("cannot register child identity".into()),
            probe_failures: vec![],
            runtime: Some("codex".into()),
        };

        assert!(!can_reuse_existing(&record));
        record.status = "starting".into();
        assert!(can_reuse_existing(&record));
    }
