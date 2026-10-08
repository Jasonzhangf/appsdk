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
