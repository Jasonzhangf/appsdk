    fn migration_writer(target_epoch: u64) -> MigrationWriterReceipt {
        MigrationWriterReceipt::new(
            "migration-1",
            "project-1",
            None,
            target_epoch,
            "sha256:source",
            AgentId::new("writer-1").unwrap(),
            OperationId::new("writer-op-1").unwrap(),
            7,
            1,
            1,
        )
        .map(|receipt| receipt.with_archive_digest("sha256:archive"))
        .expect("migration writer receipt")
    }

    fn migration_commit(
        operation_id: &str,
        binding_id: Option<&str>,
        committed_revision: u64,
    ) -> MigrationCommitEvidence {
        MigrationCommitEvidence::new(
            "migration-1",
            "project-1",
            2,
            "sha256:source",
            OperationId::new(operation_id).expect("operation id"),
            binding_id.map(|value| BindingId::new(value).expect("binding id")),
            7,
            committed_revision,
        )
        .map(|evidence| evidence.with_archive_digest("sha256:archive"))
        .expect("migration commit evidence")
    }

    #[test]
    fn migration_receipt_set_requires_one_writer_and_unique_rebind_pairs() {
        let missing = MigrationReceiptSet::default();
        assert!(matches!(
            missing.validate(),
            Err(StateError::Invalid {
                field: "migration writer receipt",
                ..
            })
        ));

        let mut writer = migration_writer(2);
        writer.writer_count = 2;
        assert!(writer.validate().is_err());

        let mut receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            ..MigrationReceiptSet::default()
        };
        let scope = project_scope();
        let identity = MigrationIdentityRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            None,
            2,
            "sha256:source",
            AgentId::new("agent-1").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            OperationId::new("identity-op-1").unwrap(),
            7,
            2,
        )
        .unwrap();
        let runtime = MigrationRuntimeRebindReceipt::new(
            "migration-1",
            "project-1",
            scope,
            None,
            2,
            "sha256:source",
            AgentId::new("agent-other").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            None,
            OperationId::new("runtime-op-1").unwrap(),
            7,
            3,
        )
        .unwrap();
        receipts.identity_rebinds.push(identity);
        receipts.runtime_rebinds.push(runtime);
        assert!(matches!(
            receipts.validate(),
            Err(StateError::Invariant(reason)) if reason.contains("matching runtime receipt")
        ));
    }

    #[test]
    fn global_state_checks_migration_runtime_receipts_against_target_bindings() {
        let scope = project_scope();
        let mut state = GlobalState::new(2).unwrap();
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
                1,
            ))
            .unwrap();

        let mut receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            ..MigrationReceiptSet::default()
        };
        receipts.identity_rebinds.push(
            MigrationIdentityRebindReceipt::new(
                "migration-1",
                "project-1",
                scope.clone(),
                None,
                2,
                "sha256:source",
                AgentId::new("agent-one").unwrap(),
                app_scope("app-one"),
                RuntimeId::new("runtime-one").unwrap(),
                BindingId::new("binding-one").unwrap(),
                1,
                OperationId::new("identity-op-1").unwrap(),
                7,
                2,
            )
            .unwrap(),
        );
        receipts.runtime_rebinds.push(
            MigrationRuntimeRebindReceipt::new(
                "migration-1",
                "project-1",
                scope,
                None,
                2,
                "sha256:source",
                AgentId::new("agent-one").unwrap(),
                app_scope("app-one"),
                RuntimeId::new("runtime-one").unwrap(),
                BindingId::new("binding-one").unwrap(),
                1,
                None,
                OperationId::new("runtime-op-1").unwrap(),
                7,
                3,
            )
            .unwrap(),
        );
        state.set_counters(3, 3);
        state.migration_commit_evidence.insert(
            "writer-op-1".into(),
            migration_commit("writer-op-1", None, 1),
        );
        state.migration_commit_evidence.insert(
            "identity-op-1".into(),
            migration_commit("identity-op-1", Some("binding-one"), 2),
        );
        state.migration_commit_evidence.insert(
            "runtime-op-1".into(),
            migration_commit("runtime-op-1", Some("binding-one"), 3),
        );
        state.validate_migration_receipts(&receipts).unwrap();

        let runtime_evidence = state
            .migration_commit_evidence
            .remove("runtime-op-1")
            .expect("runtime commit evidence");
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason))
                if reason.contains("has no authoritative migration commit evidence")
        ));
        state
            .migration_commit_evidence
            .insert("runtime-op-1".into(), runtime_evidence);

        state
            .migration_commit_evidence
            .get_mut("runtime-op-1")
            .expect("runtime commit evidence")
            .committed_revision = 4;
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason)) if reason.contains("exceeds global revision")
        ));
        state
            .migration_commit_evidence
            .get_mut("runtime-op-1")
            .expect("runtime commit evidence")
            .committed_revision = 3;

        state
            .migration_commit_evidence
            .get_mut("runtime-op-1")
            .expect("runtime commit evidence")
            .fencing_token = 8;
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason)) if reason.contains("evidence uses fencing token")
        ));
        state
            .migration_commit_evidence
            .get_mut("runtime-op-1")
            .expect("runtime commit evidence")
            .fencing_token = 7;

        receipts.runtime_rebinds[0].native_thread_id =
            Some(NativeThreadId::new("native-thread-one").expect("native thread id"));
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason)) if reason.contains("no matching migration receipt")
        ));
        receipts.runtime_rebinds[0].native_thread_id = None;

        receipts.identity_rebinds[0].endpoint_generation = 2;
        receipts.runtime_rebinds[0].endpoint_generation = 2;
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason)) if reason.contains("no matching migration receipt")
        ));
    }

    #[test]
    fn migration_receipts_require_one_source_epoch_and_a_symmetric_pairing() {
        assert!(MigrationWriterReceipt::new(
            "migration-1",
            "project-1",
            Some(0),
            2,
            "sha256:source",
            AgentId::new("writer-1").unwrap(),
            OperationId::new("writer-op-1").unwrap(),
            7,
            1,
            1,
        )
        .is_err());

        let scope = project_scope();
        let identity = MigrationIdentityRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            None,
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            OperationId::new("identity-op-1").unwrap(),
            7,
            2,
        )
        .unwrap();
        let duplicate_identity = MigrationIdentityRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            None,
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            OperationId::new("identity-op-2").unwrap(),
            7,
            3,
        )
        .unwrap();
        let runtime = MigrationRuntimeRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            None,
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            None,
            OperationId::new("runtime-op-1").unwrap(),
            7,
            4,
        )
        .unwrap();
        let unmatched_runtime = MigrationRuntimeRebindReceipt::new(
            "migration-1",
            "project-1",
            scope,
            None,
            2,
            "sha256:source",
            AgentId::new("agent-two").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-two").unwrap(),
            BindingId::new("binding-two").unwrap(),
            1,
            None,
            OperationId::new("runtime-op-2").unwrap(),
            7,
            5,
        )
        .unwrap();
        let receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            identity_rebinds: vec![identity, duplicate_identity],
            runtime_rebinds: vec![runtime, unmatched_runtime],
        };

        assert!(matches!(
            receipts.validate(),
            Err(StateError::Invariant(reason))
                if reason.contains("no unique matching identity receipt")
        ));
    }

    #[test]
    fn migration_receipts_reject_a_rebind_with_a_different_source_epoch() {
        let scope = project_scope();
        let identity = MigrationIdentityRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            Some(1),
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            OperationId::new("identity-op-1").unwrap(),
            7,
            2,
        )
        .unwrap();
        let runtime = MigrationRuntimeRebindReceipt::new(
            "migration-1",
            "project-1",
            scope,
            Some(1),
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            None,
            OperationId::new("runtime-op-1").unwrap(),
            7,
            3,
        )
        .unwrap();
        let receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            identity_rebinds: vec![identity],
            runtime_rebinds: vec![runtime],
        };

        assert!(matches!(
            receipts.validate(),
            Err(StateError::Invariant(reason)) if reason.contains("different source epoch")
        ));
    }

    /// The receipt, not the record, owns the archive digest.  A record-only
    /// edit (the exact attack the reviewer reproduced) must be refused.
    #[test]
    fn migration_commit_fence_rejects_a_record_only_archive_digest_edit() {
        let mut state = GlobalState::new(2).unwrap();
        state.set_counters(9, 9);
        state.migration_commit_evidence.insert(
            "writer-op-1".into(),
            migration_commit("writer-op-1", None, 1)
                .with_archive_digest("sha256:attacker-supplied"),
        );
        let receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            ..MigrationReceiptSet::default()
        };

        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason))
                if reason.contains("evidence archive digest sha256:attacker-supplied does not match the receipt archive digest sha256:archive")
        ));
    }

    /// A pre-fence record deserialized through `#[serde(default)]` has an empty
    /// archive digest and must be refused by the gate, not accepted.
    #[test]
    fn migration_commit_fence_rejects_an_unfenced_legacy_record_at_the_gate() {
        let mut state = GlobalState::new(2).unwrap();
        state.set_counters(9, 9);
        state.migration_commit_evidence.insert(
            "writer-op-1".into(),
            MigrationCommitEvidence::new(
                "migration-1",
                "project-1",
                2,
                "sha256:source",
                OperationId::new("writer-op-1").expect("operation id"),
                None,
                7,
                1,
            )
            .expect("legacy unfenced evidence deserializes"),
        );
        let receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            ..MigrationReceiptSet::default()
        };

        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason))
                if reason.contains("evidence archive digest  does not match the receipt archive digest sha256:archive")
        ));
    }

    /// A receipt that carries no authoritative digest must be refused before
    /// any evidence comparison, so the fence fails closed on both sides.
    #[test]
    fn migration_commit_fence_rejects_a_receipt_without_an_archive_digest() {
        let mut state = GlobalState::new(2).unwrap();
        state.set_counters(9, 9);
        state.migration_commit_evidence.insert(
            "writer-op-1".into(),
            migration_commit("writer-op-1", None, 1),
        );
        let receipts = MigrationReceiptSet {
            writer: Some(
                MigrationWriterReceipt::new(
                    "migration-1",
                    "project-1",
                    None,
                    2,
                    "sha256:source",
                    AgentId::new("writer-1").unwrap(),
                    OperationId::new("writer-op-1").unwrap(),
                    7,
                    1,
                    1,
                )
                .expect("legacy receipt deserializes"),
            ),
            ..MigrationReceiptSet::default()
        };

        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason))
                if reason.contains("carries no authoritative archive digest")
        ));
    }
