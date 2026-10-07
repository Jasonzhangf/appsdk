    use super::*;
    use serde_json::Value;
    use std::path::Path;

    #[test]
    fn recording_many_ledger_scan_receipts_does_not_copy_global_state_each_time() {
        // A durable scan receipt is appended for every ledger scan that finds
        // blocked work, so a real project journal can hold tens of thousands of
        // them.  Recording one must not deep-clone the whole global state, or
        // journal replay becomes quadratic and the daemon never becomes ready.
        let mut state = GlobalState::default();
        let receipts = 30_000_u32;
        let started = std::time::Instant::now();
        for index in 0..receipts {
            state
                .record_ledger_scan_receipt(LedgerScanReceipt {
                    scan_id: format!("ledger-scan-{index}"),
                    scanned_ms: i64::from(index),
                    classified: 0,
                    transitioned: 0,
                    unchanged: 7,
                    blocked: 93,
                    mailbox_messages_unchanged: true,
                })
                .unwrap();
        }
        let elapsed = started.elapsed();
        assert_eq!(state.ledger_scan_receipts.len(), receipts as usize);
        assert_eq!(state.sequence, u64::from(receipts));
        state.validate().unwrap();
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "recording {receipts} scan receipts took {elapsed:?}"
        );
    }
include!("global_state_tests_part2.rs");

    fn project_scope() -> ProjectScopeId {
        GlobalState::canonical_project_scope(Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("repository root is canonical")
    }

    fn app_scope(id: &str) -> AppServerId {
        AppServerId::new(id).expect("app scope id")
    }

    fn registration(scope: &ProjectScopeId, app: &str) -> ProjectRegistration {
        ProjectRegistration::new(scope.clone(), app_scope(app)).expect("registration")
    }

    fn binding(
        scope: &ProjectScopeId,
        app: &str,
        agent: &str,
        runtime: &str,
        binding_id: &str,
        generation: u64,
    ) -> RuntimeBinding {
        RuntimeBinding::new(
            scope.clone(),
            app_scope(app),
            AgentId::new(agent).unwrap(),
            RuntimeId::new(runtime).unwrap(),
            BindingId::new(binding_id).unwrap(),
            generation,
            None,
        )
        .expect("binding")
    }

    fn grant(
        scope: &ProjectScopeId,
        app: &str,
        agent: &str,
        binding_id: &str,
        generation: u64,
    ) -> MasterGrant {
        MasterGrant::new(
            scope.clone(),
            app_scope(app),
            AgentId::new(agent).unwrap(),
            "task-scoped",
            "operator",
            "user approved",
            BindingId::new(binding_id).unwrap(),
            generation,
            1,
        )
        .expect("master grant")
    }

    #[test]
    fn different_projects_are_stored_without_overwriting_each_other() {
        let root = project_scope();
        let second = ProjectScopeId::new(format!("{}/second", root.as_str())).unwrap();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&root, "app-one"))
            .unwrap();
        state
            .register_project(registration(&second, "app-two"))
            .unwrap();

        assert_eq!(state.projects.len(), 2);
        assert!(state.lookup_project(&root).is_some());
        assert!(state.lookup_project(&second).is_some());
        state.validate().unwrap();
    }

    #[test]
    fn route_registration_is_idempotent_and_conflicts_keep_the_original() {
        let scope = project_scope();
        let route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: scope.clone(),
        };
        let mut state = GlobalState::default();
        let first = state
            .register_project_for_route(&route, 11)
            .expect("first route registration");
        let replay = state
            .register_project_for_route(&route, 11)
            .expect("identical route registration is idempotent");
        assert_eq!(replay, first);
        assert_eq!(state.projects.len(), 1);

        let conflict = state.register_project_for_route(&route, 12);
        assert!(matches!(
            conflict,
            Err(StateError::RegistrationConflict {
                project_scope,
                app_scope_id
            }) if project_scope == scope.as_str() && app_scope_id == "app-one"
        ));
        assert_eq!(
            state
                .lookup_registration(&scope, &route.app_scope_id)
                .unwrap()
                .registered_at_ms,
            11
        );
        state.validate().unwrap();
    }

    #[test]
    fn route_lookup_rejects_unknown_project_or_app_scope() {
        let scope = project_scope();
        let unknown_project =
            ProjectScopeId::new(format!("{}/unknown", scope.as_str())).expect("scope");
        let known_route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: scope.clone(),
        };
        let unknown_app_route = RouteScope {
            app_scope_id: app_scope("app-unknown"),
            project_scope_id: scope.clone(),
        };
        let unknown_project_route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: unknown_project.clone(),
        };
        let mut state = GlobalState::default();
        state
            .register_project_for_route(&known_route, 1)
            .expect("known route registration");

        assert!(state.lookup_project_for_route(&unknown_app_route).is_none());
        assert!(state
            .lookup_project_for_route(&unknown_project_route)
            .is_none());
        let binding = binding(
            &unknown_project,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            1,
        );
        let before = state.version();
        assert!(matches!(
            state.bind_runtime(binding),
            Err(StateError::ProjectNotRegistered(_))
        ));
        assert_eq!(state.version(), before);
        state.validate().unwrap();
    }

    #[test]
    fn same_project_different_app_scopes_keep_separate_registrations_and_bindings() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        state
            .register_project(registration(&scope, "app-two"))
            .unwrap();
        state
            .bind_runtime(binding(
                &scope,
                "app-one",
                "agent-one",
                "runtime-one",
                "binding-one",
                1,
            ))
            .unwrap();
        state
            .bind_runtime(binding(
                &scope,
                "app-two",
                "agent-two",
                "runtime-two",
                "binding-two",
                1,
            ))
            .unwrap();

        let project = state.lookup_project(&scope).unwrap();
        assert_eq!(project.registrations.len(), 2);
        assert_eq!(project.runtime_bindings.len(), 2);
        assert_eq!(
            project.registrations["app-one"].app_scope_id.as_str(),
            "app-one"
        );
        assert_eq!(
            project.registrations["app-two"].app_scope_id.as_str(),
            "app-two"
        );
        assert_eq!(
            state
                .lookup_binding_for(
                    &binding(
                        &scope,
                        "app-two",
                        "agent-two",
                        "runtime-two",
                        "binding-two",
                        1
                    )
                    .route_scope(),
                    &BindingId::new("binding-two").unwrap()
                )
                .unwrap()
                .app_scope_id
                .as_str(),
            "app-two"
        );
        state.validate().unwrap();
    }

    #[test]
    fn same_binding_id_in_different_projects_is_route_scoped() {
        let first_scope = project_scope();
        let second_scope = ProjectScopeId::new(format!("{}/second", first_scope.as_str()))
            .expect("second project scope");
        let mut state = GlobalState::default();
        state
            .register_project(registration(&first_scope, "app-one"))
            .unwrap();
        state
            .register_project(registration(&second_scope, "app-one"))
            .unwrap();

        let first = binding(
            &first_scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "shared-binding",
            1,
        );
        let second = binding(
            &second_scope,
            "app-one",
            "agent-two",
            "runtime-two",
            "shared-binding",
            1,
        );
        state.bind_runtime(first.clone()).unwrap();
        state.bind_runtime(second.clone()).unwrap();

        let shared_id = BindingId::new("shared-binding").unwrap();
        assert!(state.lookup_binding(&shared_id).is_none());
        assert_eq!(
            state.lookup_binding_for(&first.route_scope(), &shared_id),
            Some(&first)
        );
        assert_eq!(
            state.lookup_binding_for(&second.route_scope(), &shared_id),
            Some(&second)
        );
        state.validate_binding(&first).unwrap();
        state.validate_binding(&second).unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn master_grants_are_isolated_by_project_route_and_generation() {
        let first_scope = project_scope();
        let second_scope = ProjectScopeId::new(format!("{}/second", first_scope.as_str()))
            .expect("second project scope");
        let first_route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: first_scope.clone(),
        };
        let second_route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: second_scope.clone(),
        };
        let mut state = GlobalState::default();
        state.register_project_for_route(&first_route, 1).unwrap();
        state.register_project_for_route(&second_route, 2).unwrap();
        let first_binding = binding(
            &first_scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "shared-binding",
            3,
        );
        let second_binding = binding(
            &second_scope,
            "app-one",
            "agent-two",
            "runtime-two",
            "shared-binding",
            4,
        );
        state.bind_runtime(first_binding.clone()).unwrap();
        state.bind_runtime(second_binding.clone()).unwrap();
        state
            .grant_master(grant(
                &first_scope,
                "app-one",
                "agent-one",
                "shared-binding",
                3,
            ))
            .unwrap();
        state
            .grant_master(grant(
                &second_scope,
                "app-one",
                "agent-two",
                "shared-binding",
                4,
            ))
            .unwrap();

        let shared_id = BindingId::new("shared-binding").unwrap();
        assert_eq!(
            state
                .lookup_master_grant_for(&first_route, &shared_id)
                .unwrap()
                .project_scope,
            first_scope
        );
        assert_eq!(
            state
                .lookup_master_grant_for(&second_route, &shared_id)
                .unwrap()
                .project_scope,
            second_scope
        );
        assert_eq!(
            state.role_for_route(&first_route, &shared_id),
            PeerRole::Master
        );
        assert_eq!(
            state.role_for_route(&second_route, &shared_id),
            PeerRole::Master
        );
        let wrong_route = RouteScope {
            app_scope_id: app_scope("app-unknown"),
            project_scope_id: first_scope,
        };
        assert_eq!(
            state.role_for_route(&wrong_route, &shared_id),
            PeerRole::Peer
        );
        state.validate().unwrap();
    }

    #[test]
    fn sequence_and_revision_advance_together_and_cas_is_fenced() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        assert_eq!(
            state.version(),
            StateVersion {
                epoch: 1,
                sequence: 0,
                revision: 0
            }
        );
        let first = state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let second = state
            .bind_runtime(binding(
                &scope,
                "app-one",
                "agent-one",
                "runtime-one",
                "binding-one",
                1,
            ))
            .unwrap();
        assert_eq!((first.sequence, first.revision), (1, 1));
        assert_eq!((second.sequence, second.revision), (2, 2));

        let third = state
            .compare_and_swap(second.revision, |next| {
                next.projects
                    .get_mut(scope.as_str())
                    .unwrap()
                    .registrations
                    .get_mut("app-one")
                    .unwrap()
                    .registered_at_ms = 7;
                Ok(())
            })
            .unwrap();
        assert_eq!((third.sequence, third.revision), (3, 3));
        assert!(matches!(
            state.compare_and_swap(2, |_| Ok(())),
            Err(StateError::CompareAndSwapMismatch {
                expected: 2,
                observed: 3
            })
        ));
        state.validate().unwrap();
    }

    #[test]
    fn old_generation_is_rejected_without_mutating_state() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let current = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            2,
        );
        state.bind_runtime(current.clone()).unwrap();
        let before = state.version();
        let stale = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            1,
        );
        assert!(matches!(
            state.validate_binding(&stale),
            Err(StateError::StaleBinding {
                expected_generation: 2,
                observed_generation: 1,
                ..
            })
        ));
        assert!(matches!(
            state.bind_runtime(stale),
            Err(StateError::StaleBinding {
                expected_generation: 2,
                observed_generation: 1,
                ..
            })
        ));
        assert_eq!(state.version(), before);
        assert_eq!(
            state
                .lookup_binding(&current.binding_id)
                .unwrap()
                .endpoint_generation,
            2
        );
    }

    #[test]
    fn registration_defaults_to_peer_until_an_explicit_current_grant() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let runtime = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            4,
        );
        state.bind_runtime(runtime.clone()).unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &runtime.binding_id),
            PeerRole::Peer
        );

        state
            .grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 4))
            .unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &runtime.binding_id),
            PeerRole::Master
        );
        state.validate().unwrap();
    }

    #[test]
    fn route_rejects_a_second_master_grant() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let first = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            1,
        );
        let second = binding(
            &scope,
            "app-one",
            "agent-two",
            "runtime-two",
            "binding-two",
            1,
        );
        state.bind_runtime(first.clone()).unwrap();
        state.bind_runtime(second.clone()).unwrap();
        state
            .grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 1))
            .unwrap();

        assert!(matches!(
            state.grant_master(grant(&scope, "app-one", "agent-two", "binding-two", 1,)),
            Err(StateError::MasterGrantConflict(_))
        ));
        assert_eq!(
            state.role_for_binding(&scope, &first.binding_id),
            PeerRole::Master
        );
        assert_eq!(
            state.role_for_binding(&scope, &second.binding_id),
            PeerRole::Peer
        );
        state.validate().unwrap();
    }

    #[test]
    fn reconnect_revokes_old_master_grant_and_explicit_revoke_is_idempotent() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let current = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            4,
        );
        state.bind_runtime(current.clone()).unwrap();
        state
            .grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 4))
            .unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &current.binding_id),
            PeerRole::Master
        );

        let reconnected = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            5,
        );
        state.bind_runtime(reconnected.clone()).unwrap();
        assert!(state
            .lookup_master_grant(&scope, &reconnected.binding_id)
            .is_none());
        assert_eq!(
            state.role_for_binding(&scope, &reconnected.binding_id),
            PeerRole::Peer
        );
        assert!(matches!(
            state.grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 4)),
            Err(StateError::StaleBinding {
                expected_generation: 5,
                observed_generation: 4,
                ..
            })
        ));
        state
            .grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 5))
            .unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &reconnected.binding_id),
            PeerRole::Master
        );

        let before_revoke = state.version();
        state
            .revoke_master(&scope, &reconnected.binding_id)
            .unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &reconnected.binding_id),
            PeerRole::Peer
        );
        assert!(state
            .lookup_master_grant(&scope, &reconnected.binding_id)
            .is_none());
        let after_revoke = state.version();
        assert!(after_revoke.revision > before_revoke.revision);
        state
            .revoke_master(&scope, &reconnected.binding_id)
            .unwrap();
        assert_eq!(state.version(), after_revoke);
        state.validate_binding(&reconnected).unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn runtime_binding_rollback_restores_only_the_exact_previous_generation_and_grant() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let previous = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            4,
        );
        let previous_grant = grant(&scope, "app-one", "agent-one", "binding-one", 4);
        state.bind_runtime(previous.clone()).unwrap();
        state.grant_master(previous_grant.clone()).unwrap();
        let failed = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-two",
            "binding-one",
            5,
        );
        state.bind_runtime(failed.clone()).unwrap();
        // A same-principal recovery reissues the grant for the new
        // generation, which is exactly the transaction rollback must undo.
        let failed_grant = grant(&scope, "app-one", "agent-one", "binding-one", 5);
        state.grant_master(failed_grant.clone()).unwrap();
        assert_eq!(
            state.lookup_master_grant(&scope, &failed.binding_id),
            Some(&failed_grant)
        );

        state
            .rollback_runtime_binding(
                failed.clone(),
                Some(previous.clone()),
                Some(previous_grant.clone()),
            )
            .unwrap();
        assert_eq!(state.lookup_binding(&previous.binding_id), Some(&previous));
        assert_eq!(
            state.lookup_master_grant(&scope, &previous.binding_id),
            Some(&previous_grant)
        );
        assert_eq!(
            state.role_for_binding(&scope, &previous.binding_id),
            PeerRole::Master
        );
        state.validate().unwrap();

        let before = state.version();
        assert!(matches!(
            state.rollback_runtime_binding(
                failed,
                Some(previous.clone()),
                Some(previous_grant.clone()),
            ),
            Err(StateError::BindingConflict(_))
        ));
        assert_eq!(state.version(), before);
    }

    #[test]
    fn runtime_binding_rollback_rejects_a_grant_that_is_not_the_failed_transaction_state() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let previous = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            4,
        );
        state.bind_runtime(previous.clone()).unwrap();
        let failed = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-two",
            "binding-one",
            5,
        );
        state.bind_runtime(failed.clone()).unwrap();

        // A grant on the same binding whose generation does not match the
        // failed transaction is not this transaction's state, so rollback
        // must still fail closed instead of silently restoring it.
        let mismatched = MasterGrant::new(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            "project",
            "operator",
            "user approved",
            BindingId::new("binding-one").unwrap(),
            failed.endpoint_generation + 1,
            1,
        )
        .unwrap();
        let mut with_mismatched = state.clone();
        with_mismatched
            .projects
            .get_mut(scope.as_str())
            .unwrap()
            .master_grants
            .insert(failed.binding_id.as_str().to_owned(), mismatched);
        assert!(matches!(
            with_mismatched.rollback_runtime_binding(failed.clone(), Some(previous), None),
            Err(StateError::MasterGrantConflict(_))
        ));
        assert_eq!(with_mismatched.version(), state.version());
        state.validate().unwrap();
    }

    #[test]
    fn legacy_thread_route_is_indexed_read_only_and_upgraded_by_a_strict_route() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-legacy-upgrade").unwrap();
        let legacy = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-legacy").unwrap(),
            RuntimeId::new("runtime-legacy").unwrap(),
            BindingId::new("binding-legacy").unwrap(),
            1,
            None,
            Some(thread_id.clone()),
        )
        .unwrap();

        state.bind_runtime(legacy.clone()).unwrap();
        state.set_legacy_thread_route(legacy.clone()).unwrap();
        assert_eq!(state.legacy_thread_route_matches(&thread_id), vec![&legacy]);
        assert!(state
            .lookup_current_thread_route(
                &SessionId::new("session-legacy-upgrade").unwrap(),
                &thread_id
            )
            .is_none());
        state.validate().unwrap();

        // A strict dual-key route for the same thread is the upgrade; the
        // legacy record must not remain as a second live selector.
        let strict = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-legacy").unwrap(),
            RuntimeId::new("runtime-legacy").unwrap(),
            BindingId::new("binding-legacy").unwrap(),
            2,
            Some(SessionId::new("session-legacy-upgrade").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        state.bind_runtime(strict.clone()).unwrap();
        state.set_current_thread_route(strict.clone()).unwrap();
        assert!(state.legacy_thread_route_matches(&thread_id).is_empty());
        assert_eq!(
            state.lookup_current_thread_route(
                &SessionId::new("session-legacy-upgrade").unwrap(),
                &thread_id
            ),
            Some(&strict)
        );
        state.validate().unwrap();
    }

    #[test]
    fn legacy_thread_route_keeps_distinct_bindings_for_one_thread_as_candidates() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-legacy-candidates").unwrap();
        let first = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            None,
            Some(thread_id.clone()),
        )
        .unwrap();
        let second = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-two").unwrap(),
            RuntimeId::new("runtime-two").unwrap(),
            BindingId::new("binding-two").unwrap(),
            1,
            None,
            Some(thread_id.clone()),
        )
        .unwrap();
        state.set_legacy_thread_route(first.clone()).unwrap();
        state.set_legacy_thread_route(second.clone()).unwrap();

        // Both candidates are retained; the resolver must fail closed rather
        // than pick one.
        let mut matches = state.legacy_thread_route_matches(&thread_id);
        matches.sort_by(|left, right| left.binding_id.as_str().cmp(right.binding_id.as_str()));
        assert_eq!(matches, vec![&first, &second]);
        state.validate().unwrap();
    }

    #[test]
    fn upgrading_one_legacy_binding_keeps_the_other_candidate_resolvable() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-legacy-sibling").unwrap();
        let legacy = |agent: &str, binding_id: &str, runtime_id: &str| {
            RuntimeBinding::new_with_session(
                scope.clone(),
                app_scope("app-one"),
                AgentId::new(agent).unwrap(),
                RuntimeId::new(runtime_id).unwrap(),
                BindingId::new(binding_id).unwrap(),
                1,
                None,
                Some(thread_id.clone()),
            )
            .unwrap()
        };
        let first = legacy("agent-one", "binding-one", "runtime-one");
        let second = legacy("agent-two", "binding-two", "runtime-two");
        state.bind_runtime(first.clone()).unwrap();
        state.bind_runtime(second.clone()).unwrap();
        state.set_legacy_thread_route(first.clone()).unwrap();
        state.set_legacy_thread_route(second.clone()).unwrap();

        // Upgrade only the first identity to a strict dual-key route.  The
        // sibling binding id must stay resolvable rather than be swept away.
        let strict = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            2,
            Some(SessionId::new("session-legacy-sibling").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        state.bind_runtime(strict.clone()).unwrap();
        state.set_current_thread_route(strict.clone()).unwrap();

        assert_eq!(state.legacy_thread_route_matches(&thread_id), vec![&second]);
        assert_eq!(
            state.lookup_current_thread_route(
                &SessionId::new("session-legacy-sibling").unwrap(),
                &thread_id
            ),
            Some(&strict)
        );
        state.validate().unwrap();
    }

    #[test]
    fn a_session_bound_thread_is_reported_as_strictly_owned() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-strict-owner").unwrap();

        // A legacy record for the thread and a distinct strict binding for the
        // same thread must both be visible, so the resolver can refuse to use
        // the legacy fallback once the thread has a live strict owner.
        let legacy = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-legacy").unwrap(),
            RuntimeId::new("runtime-legacy").unwrap(),
            BindingId::new("binding-legacy").unwrap(),
            1,
            None,
            Some(thread_id.clone()),
        )
        .unwrap();
        let strict = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-strict").unwrap(),
            RuntimeId::new("runtime-strict").unwrap(),
            BindingId::new("binding-strict").unwrap(),
            1,
            Some(SessionId::new("session-strict-owner").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        state.bind_runtime(legacy.clone()).unwrap();
        state.bind_runtime(strict.clone()).unwrap();
        state.set_legacy_thread_route(legacy.clone()).unwrap();

        assert_eq!(
            state.strict_bindings_for_native_thread(&thread_id),
            vec![&strict]
        );
        assert_eq!(state.legacy_thread_route_matches(&thread_id), vec![&legacy]);
        state.validate().unwrap();
    }

    #[test]
    fn legacy_thread_route_refresh_replaces_the_same_binding() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-legacy-refresh").unwrap();
        let binding = |generation: u64| {
            RuntimeBinding::new_with_session(
                scope.clone(),
                app_scope("app-one"),
                AgentId::new("agent-one").unwrap(),
                RuntimeId::new("runtime-one").unwrap(),
                BindingId::new("binding-one").unwrap(),
                generation,
                None,
                Some(thread_id.clone()),
            )
            .unwrap()
        };
        state.set_legacy_thread_route(binding(1)).unwrap();
        state.set_legacy_thread_route(binding(5)).unwrap();
        // A lower or equal generation must not roll the record back.
        state.set_legacy_thread_route(binding(3)).unwrap();

        let matches = state.legacy_thread_route_matches(&thread_id);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].endpoint_generation, 5);
        state.validate().unwrap();
    }

    #[test]
    fn pane_only_lookup_finds_appserver_binding_by_pane_recovery_anchor() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-pane-only-appserver").unwrap();
        let mut strict = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-pane-appserver").unwrap(),
            RuntimeId::new("runtime-pane-appserver").unwrap(),
            BindingId::new("binding-pane-appserver").unwrap(),
            1,
            Some(SessionId::new("session-pane-appserver").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        strict.tmux_endpoint = Some(crate::proto::TmuxEndpoint {
            socket_path: "/tmp/collab-pane-appserver.sock".into(),
            server_pid: 11,
            tmux_session_id: "$9".into(),
            pane_id: "%33".into(),
            pane_pid: 55,
            codex_session_id: Some("session-pane-appserver".into()),
            codex_thread_id: Some("thread-pane-only-appserver".into()),
        });
        state.bind_runtime(strict.clone()).unwrap();
        state.set_current_thread_route(strict.clone()).unwrap();

        let pane_only = crate::proto::TmuxEndpoint {
            socket_path: "/tmp/collab-pane-appserver.sock".into(),
            server_pid: 11,
            tmux_session_id: "$9".into(),
            pane_id: "%33".into(),
            pane_pid: 55,
            codex_session_id: None,
            codex_thread_id: None,
        };
        assert_eq!(
            state.lookup_tmux_route(&pane_only),
            Some(&strict),
            "pane-only lookup must find the App Server route's persisted pane recovery anchor"
        );
        let changed_thread = crate::proto::TmuxEndpoint {
            codex_session_id: Some("session-new".into()),
            codex_thread_id: Some("thread-new".into()),
            ..pane_only
        };
        assert_eq!(
            state.lookup_unique_tmux_pane_route(&changed_thread),
            Some(&strict),
            "recovery must resolve a unique complete pane independent of Codex IDs"
        );
        state.validate().unwrap();
    }

    #[test]
    fn current_thread_routes_keep_distinct_sessions_for_one_native_thread() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-shared").unwrap();
        let first = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            Some(SessionId::new("session-one").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        let second = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-two").unwrap(),
            RuntimeId::new("runtime-two").unwrap(),
            BindingId::new("binding-two").unwrap(),
            1,
            Some(SessionId::new("session-two").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();

        state.bind_runtime(first.clone()).unwrap();
        state.bind_runtime(second.clone()).unwrap();
        state.set_current_thread_route(first.clone()).unwrap();
        state.set_current_thread_route(second.clone()).unwrap();

        assert_eq!(
            state.lookup_current_thread_route(first.session_id.as_ref().unwrap(), &thread_id,),
            Some(&first)
        );
        assert_eq!(
            state.lookup_current_thread_route(second.session_id.as_ref().unwrap(), &thread_id,),
            Some(&second)
        );
        state.validate().unwrap();
    }

    #[test]
    fn same_address_generation_refresh_replaces_the_route_without_a_tombstone() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-refresh").unwrap();
        let session_id = SessionId::new("session-refresh").unwrap();
        let first = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            Some(session_id.clone()),
            Some(thread_id.clone()),
        )
        .unwrap();
        let refreshed = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            2,
            Some(session_id.clone()),
            Some(thread_id.clone()),
        )
        .unwrap();

        state.bind_runtime(first.clone()).unwrap();
        state.set_current_thread_route(first).unwrap();
        state.bind_runtime(refreshed.clone()).unwrap();
        state.set_current_thread_route(refreshed.clone()).unwrap();

        assert_eq!(
            state.lookup_current_thread_route(&session_id, &thread_id),
            Some(&refreshed)
        );
        assert!(state
            .lookup_current_thread_route_tombstone(&session_id, &thread_id)
            .is_none());
        state.validate().unwrap();
    }

    /// Builds a route for `binding_id` at a distinct session/thread address.
    fn route_at(
        scope: &ProjectScopeId,
        app: &str,
        runtime: &str,
        binding_id: &str,
        generation: u64,
        session: &str,
        thread: &str,
    ) -> RuntimeBinding {
        RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope(app),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new(runtime).unwrap(),
            BindingId::new(binding_id).unwrap(),
            generation,
            Some(SessionId::new(session).unwrap()),
            Some(NativeThreadId::new(thread).unwrap()),
        )
        .unwrap()
    }

    #[test]
    fn an_orphan_incumbent_route_does_not_block_a_lower_generation_takeover() {
        // A host index can keep a pane route for a binding its project runtime
        // no longer holds.  Nothing can resolve that route, so it must not gate
        // a fresh registration: the daemon takes the pane by default.  Before
        // this rule the takeover failed with StaleBinding and the daemon could
        // not restart afterwards.
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let orphan = route_at(
            &scope,
            "app-one",
            "runtime-old",
            "binding-orphan",
            7,
            "session-orphan-old",
            "thread-orphan-old",
        );
        // Deliberately NOT bound into the project's runtime bindings.  This is
        // the orphan shape the host index kept for pane %4.
        state.set_current_thread_route(orphan).unwrap();

        let takeover = route_at(
            &scope,
            "app-one",
            "runtime-new",
            "binding-orphan",
            1,
            "session-orphan-new",
            "thread-orphan-new",
        );
        state.set_current_thread_route(takeover.clone()).unwrap();

        assert_eq!(
            state.lookup_current_thread_route(
                takeover.session_id.as_ref().unwrap(),
                takeover.native_thread_id.as_ref().unwrap(),
            ),
            Some(&takeover)
        );
        assert!(state
            .lookup_current_thread_route(
                &SessionId::new("session-orphan-old").unwrap(),
                &NativeThreadId::new("thread-orphan-old").unwrap(),
            )
            .is_none());
        assert!(state
            .lookup_current_thread_route_tombstone(
                &SessionId::new("session-orphan-old").unwrap(),
                &NativeThreadId::new("thread-orphan-old").unwrap(),
            )
            .is_none());
        state.validate().unwrap();
    }

    #[test]
    fn a_live_incumbent_route_still_rejects_a_lower_generation_takeover() {
        // The protection C8 must not weaken: when this reducer DOES hold the
        // incumbent's runtime binding, the incumbent is live, so a lower
        // generation still fails closed.
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let live = route_at(
            &scope,
            "app-one",
            "runtime-live",
            "binding-live",
            7,
            "session-live-old",
            "thread-live-old",
        );
        state.bind_runtime(live.clone()).unwrap();
        state.set_current_thread_route(live).unwrap();

        let takeover = route_at(
            &scope,
            "app-one",
            "runtime-live-new",
            "binding-live",
            1,
            "session-live-new",
            "thread-live-new",
        );
        assert!(matches!(
            state.set_current_thread_route(takeover),
            Err(StateError::StaleBinding {
                expected_generation: 7,
                observed_generation: 1,
                ..
            })
        ));
        state.validate().unwrap();
    }

    #[test]
    fn an_equal_generation_address_change_is_still_rejected_for_a_live_incumbent() {
        // "An address change must raise the generation" is the second invariant
        // C8 must not weaken.  Both owners enforce it: the route tombstone
        // requires a strictly greater generation, and the runtime ledger
        // rejects an equal generation as a different runtime at the same
        // generation.
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let live = route_at(
            &scope,
            "app-one",
            "runtime-eq",
            "binding-eq",
            7,
            "session-eq-old",
            "thread-eq-old",
        );
        state.bind_runtime(live.clone()).unwrap();
        state.set_current_thread_route(live).unwrap();

        let equal_route = route_at(
            &scope,
            "app-one",
            "runtime-eq",
            "binding-eq",
            7,
            "session-eq-new",
            "thread-eq-new",
        );
        assert!(matches!(
            state.set_current_thread_route(equal_route),
            Err(StateError::StaleBinding {
                expected_generation: 7,
                observed_generation: 7,
                ..
            })
        ));
        assert!(matches!(
            state.bind_runtime(route_at(
                &scope,
                "app-one",
                "runtime-eq-other",
                "binding-eq",
                7,
                "session-eq-old",
                "thread-eq-old",
            )),
            Err(StateError::BindingConflict(_))
        ));
        state.validate().unwrap();
    }

    #[test]
    fn a_live_incumbent_route_still_tombstones_a_higher_generation_takeover() {
        // The normal upgrade path keeps its retirement record, so a caller on
        // the retired address still gets the explicit stale error.
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let live = route_at(
            &scope,
            "app-one",
            "runtime-live",
            "binding-live",
            7,
            "session-upgrade-old",
            "thread-upgrade-old",
        );
        state.bind_runtime(live.clone()).unwrap();
        state.set_current_thread_route(live).unwrap();

        let takeover = route_at(
            &scope,
            "app-one",
            "runtime-live",
            "binding-live",
            8,
            "session-upgrade-new",
            "thread-upgrade-new",
        );
        state.bind_runtime(takeover.clone()).unwrap();
        state.set_current_thread_route(takeover).unwrap();

        let tombstone = state
            .lookup_current_thread_route_tombstone(
                &SessionId::new("session-upgrade-old").unwrap(),
                &NativeThreadId::new("thread-upgrade-old").unwrap(),
            )
            .expect("a live incumbent keeps its tombstone");
        assert_eq!(tombstone.rebound_to.endpoint_generation, 8);
        state.validate().unwrap();
    }

    #[test]
    fn an_unregistered_project_incumbent_is_the_declared_non_resident_boundary() {
        // The orphan criterion asks THIS reducer's project map.  The host
        // reducer holds no entry at all for a non-resident project, so it
        // cannot tell a live incumbent from an orphan there and skips the
        // tombstone.  This test pins that declared boundary rather than leaving
        // it to inference.  Generation monotonicity for such a project is owned
        // by bind_runtime in the owning runtime.
        let scope = project_scope();
        let mut state = GlobalState::default();
        let incumbent = route_at(
            &scope,
            "app-one",
            "runtime-non-resident",
            "binding-non-resident",
            7,
            "session-nr-old",
            "thread-nr-old",
        );
        state.set_current_thread_route(incumbent).unwrap();

        let takeover = route_at(
            &scope,
            "app-one",
            "runtime-non-resident-new",
            "binding-non-resident",
            1,
            "session-nr-new",
            "thread-nr-new",
        );
        state.set_current_thread_route(takeover).unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn bind_runtime_still_rejects_a_lower_generation_over_a_live_binding() {
        // The generation monotonicity owner is the owning runtime's ledger, not
        // the route tombstone, so C8 leaves it untouched.
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        state
            .bind_runtime(binding(
                &scope,
                "app-one",
                "agent-one",
                "runtime-one",
                "binding-one",
                7,
            ))
            .unwrap();
        assert!(matches!(
            state.bind_runtime(binding(
                &scope,
                "app-one",
                "agent-one",
                "runtime-one",
                "binding-one",
                1,
            )),
            Err(StateError::StaleBinding {
                expected_generation: 7,
                observed_generation: 1,
                ..
            })
        ));
        state.validate().unwrap();
    }

    #[test]
    fn rebinding_an_address_after_tombstoning_drops_the_stale_tombstone() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let first_session = SessionId::new("session-first").unwrap();
        let first_thread = NativeThreadId::new("thread-first").unwrap();
        let second_session = SessionId::new("session-second").unwrap();
        let second_thread = NativeThreadId::new("thread-second").unwrap();
        let first = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            Some(first_session.clone()),
            Some(first_thread.clone()),
        )
        .unwrap();
        let second = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            2,
            Some(second_session.clone()),
            Some(second_thread.clone()),
        )
        .unwrap();
        let rebound = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            3,
            Some(first_session.clone()),
            Some(first_thread.clone()),
        )
        .unwrap();

        state.bind_runtime(first.clone()).unwrap();
        state.set_current_thread_route(first).unwrap();
        state.bind_runtime(second.clone()).unwrap();
        state.set_current_thread_route(second).unwrap();
        state.bind_runtime(rebound.clone()).unwrap();
        state.set_current_thread_route(rebound).unwrap();

        assert!(state
            .lookup_current_thread_route_tombstone(&first_session, &first_thread)
            .is_none());
        state.validate().unwrap();
    }

    #[test]
    fn runtime_binding_ledger_replaces_stale_generation_for_the_same_binding() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let first = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            1,
        );
        let second = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            2,
        );
        state.bind_runtime(first).unwrap();
        let old_record = RuntimeBindingLedgerRecord {
            project_scope: scope.clone(),
            app_scope_id: app_scope("app-one"),
            agent_id: AgentId::new("agent-one").unwrap(),
            runtime_id: RuntimeId::new("runtime-one").unwrap(),
            binding_id: BindingId::new("binding-one").unwrap(),
            endpoint_generation: 1,
            state: RuntimeBindingLedgerState::Missing,
            probe_state: Some(RuntimeBindingLedgerState::Missing),
            reason: Some("stale generation".into()),
            classified_ms: 1,
            operation_id: OperationId::new("ledger-op-old").unwrap(),
            receipt_id: "ledger-receipt-old".into(),
        };
        state.classify_runtime_binding_ledger(old_record).unwrap();
        state.validate().unwrap();

        state.bind_runtime(second).unwrap();
        let current = RuntimeBindingLedgerRecord {
            project_scope: scope.clone(),
            app_scope_id: app_scope("app-one"),
            agent_id: AgentId::new("agent-one").unwrap(),
            runtime_id: RuntimeId::new("runtime-one").unwrap(),
            binding_id: BindingId::new("binding-one").unwrap(),
            endpoint_generation: 2,
            state: RuntimeBindingLedgerState::Live,
            probe_state: Some(RuntimeBindingLedgerState::Live),
            reason: None,
            classified_ms: 2,
            operation_id: OperationId::new("ledger-op-current").unwrap(),
            receipt_id: "ledger-receipt-current".into(),
        };
        state.classify_runtime_binding_ledger(current.clone()).unwrap();
        state.validate().unwrap();
        assert_eq!(
            state
                .lookup_runtime_binding_ledger(
                    &current.project_scope,
                    &current.app_scope_id,
                    &current.binding_id,
                )
                .unwrap()
                .endpoint_generation,
            2
        );
        assert_eq!(state.projects[scope.as_str()].runtime_binding_ledger.len(), 1);
    }

    #[test]
    fn command_receipts_are_host_wide_and_idempotent() {
        let mut state = GlobalState::default();
        let first = state
            .record_command(
                CommandId::new("command-one").unwrap(),
                OperationId::new("operation-one").unwrap(),
                serde_json::json!({"ok": true}),
            )
            .unwrap();
        let before = state.version();
        let replay = state
            .record_command(
                CommandId::new("command-one").unwrap(),
                OperationId::new("operation-one").unwrap(),
                serde_json::json!({"ok": true}),
            )
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(state.version(), before);
        assert!(matches!(
            state.record_command(
                CommandId::new("command-one").unwrap(),
                OperationId::new("operation-two").unwrap(),
                Value::Null,
            ),
            Err(StateError::CommandIdReuse { .. })
        ));
        state.validate().unwrap();
    }
