    use super::*;

    #[test]
    fn source_disposition_is_orthogonal_to_observed_mapping_class() {
        assert_eq!(
            SourceDisposition::from_mapping_class(MappingClass::Direct).as_str(),
            "direct_replay"
        );
        assert_eq!(
            SourceDisposition::from_mapping_class(MappingClass::Adapt).as_str(),
            "adapt_reconcile"
        );
        assert_eq!(
            SourceDisposition::from_mapping_class(MappingClass::Reset).as_str(),
            "rebuild_required"
        );
        assert_eq!(
            SourceDisposition::from_mapping_class(MappingClass::Unknown).as_str(),
            "archive_only"
        );
    }

    #[test]
    fn project_admission_names_are_stable_contract_values() {
        assert_eq!(ProjectAdmission::Verified.as_str(), "verified");
        assert_eq!(ProjectAdmission::ResetRequired.as_str(), "reset_required");
        assert_eq!(ProjectAdmission::NeedsOperator.as_str(), "needs_operator");
        assert_eq!(ProjectAdmission::Aborted.as_str(), "aborted");
    }

    fn direct_line(id: &str, sequence: u64) -> String {
        format!(
            "{{\"record_id\":\"{id}\",\"sequence\":{sequence},\"ev\":\"TaskUpdated\",\"task\":{{\"id\":\"task-{id}\",\"owner\":\"peer\",\"created_by\":\"master\",\"cwd\":\"/repo\",\"worktree_path\":\"playground/{id}\",\"base_commit\":\"base-{id}\",\"status\":\"working\",\"created_ms\":1,\"updated_ms\":1}}}}\n"
        )
    }

    fn canonical_options() -> InspectOptions {
        InspectOptions {
            source_path: Some(PathBuf::from("/repo/.agent-collab/server/journal.jsonl")),
            canonical_project_cwd: Some(PathBuf::from("/repo")),
            configured_worktree_base: None,
        }
    }

    fn canonical_event_bytes(
        event: Event,
        record_id: Option<&str>,
        sequence: Option<u64>,
        final_newline: bool,
    ) -> Vec<u8> {
        let mut value = serde_json::to_value(event).expect("canonical event serializes");
        if let Some(record_id) = record_id {
            value["record_id"] = Value::String(record_id.into());
        }
        if let Some(sequence) = sequence {
            value["sequence"] = Value::Number(sequence.into());
        }

        let mut bytes = serde_json::to_vec(&value).expect("canonical event value serializes");
        if final_newline {
            bytes.push(b'\n');
        }
        bytes
    }

    #[test]
    fn valid_prefix_retains_raw_bytes_and_deterministic_digest() {
        let bytes = direct_line("one", 1).into_bytes();
        let report = inspect_jsonl(&bytes);
        assert_eq!(report.classification, MappingClass::Direct);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].line_number, 1);
        assert_eq!(report.records[0].raw_bytes, bytes);
        assert_eq!(report.records[0].digest, digest_bytes(&bytes));
        assert!(report.issues.is_empty());
        assert!(report.verify_digest(&report.source_digest).is_ok());
    }

    #[test]
    fn canonical_event_without_legacy_identity_is_adapted() {
        let bytes = canonical_event_bytes(
            Event::WakeAttempted {
                ids: vec!["message-1".into()],
                attempted_ms: 42,
                retry: false,
            },
            None,
            None,
            true,
        );
        let report = inspect_jsonl(&bytes);

        assert_eq!(report.classification, MappingClass::Adapt);
        assert_eq!(report.records[0].classification, MappingClass::Adapt);
        assert!(report.has_error("LEGACY_RECORD_ID_ABSENT"));
        assert!(report.has_error("LEGACY_SEQUENCE_ABSENT"));
        assert!(!report.has_error("INVALID_EVENT"));
    }

    #[test]
    fn concatenated_canonical_events_fail_jsonl_framing_closed() {
        let first = canonical_event_bytes(
            Event::WakeAttempted {
                ids: vec!["message-1".into()],
                attempted_ms: 42,
                retry: false,
            },
            Some("event-1"),
            Some(1),
            false,
        );
        let second = canonical_event_bytes(
            Event::WakeAttempted {
                ids: vec!["message-2".into()],
                attempted_ms: 43,
                retry: false,
            },
            Some("event-2"),
            Some(2),
            false,
        );
        let mut bytes = first;
        bytes.extend_from_slice(&second);
        bytes.push(b'\n');

        let report = inspect_jsonl(&bytes);

        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("MALFORMED_JSON_TAIL"));
    }

    #[test]
    fn canonical_event_without_final_newline_is_unknown() {
        let bytes = canonical_event_bytes(
            Event::WakeAttempted {
                ids: vec!["message-1".into()],
                attempted_ms: 42,
                retry: false,
            },
            Some("event-1"),
            Some(1),
            false,
        );
        let report = inspect_jsonl(&bytes);

        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("MISSING_FINAL_NEWLINE"));
        assert!(!report.has_error("INVALID_EVENT"));
    }

    #[test]
    fn duplicate_event_tag_is_unknown_even_when_value_collapses_it() {
        let bytes = br#"{"record_id":"event-1","sequence":1,"ev":"FutureEvent","ev":"WakeAttempted","ids":["message-1"],"attempted_ms":42}
"#;
        let report = inspect_jsonl(bytes);

        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("DUPLICATE_JSON_KEY:ev"));
    }

    #[test]
    fn duplicate_canonical_event_field_is_unknown() {
        let bytes = br#"{"record_id":"event-1","sequence":1,"ev":"WakeAttempted","ids":["message-1"],"ids":["message-2"],"attempted_ms":42}
"#;
        let report = inspect_jsonl(bytes);

        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("DUPLICATE_JSON_KEY:ids"));
    }

    #[test]
    fn duplicate_envelope_key_is_unknown_even_when_event_is_valid() {
        let bytes = br#"{"record_id":"event-1","record_id":"event-2","sequence":1,"ev":"WakeAttempted","ids":["message-1"],"attempted_ms":42}
"#;
        let report = inspect_jsonl(bytes);

        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("DUPLICATE_JSON_KEY:record_id"));
    }

    #[test]
    fn empty_jsonl_line_is_unknown() {
        let report = inspect_jsonl(b"\n");

        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("EMPTY_LINE"));
    }

    #[test]
    fn canonical_cwd_accepts_bound_worktree_context() {
        let bytes = direct_line("one", 1);
        let report = inspect_jsonl_with_options(bytes.as_bytes(), &canonical_options());
        assert_eq!(report.classification, MappingClass::Direct);
        assert_eq!(
            report.source_path,
            Some(PathBuf::from("/repo/.agent-collab/server/journal.jsonl"))
        );
    }

    #[test]
    fn foreign_cwd_is_unknown_even_when_binding_fields_are_present() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"created_by\":\"master\",\"cwd\":\"/other\",\"worktree_path\":\"playground/task-1\",\"base_commit\":\"base\",\"status\":\"working\",\"created_ms\":1,\"updated_ms\":1}}\n";
        let report = inspect_jsonl_with_options(bytes, &canonical_options());
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("CWD_OUTSIDE_PROJECT_SCOPE:expected=/repo:observed=/other")
        );
        assert_eq!(
            report.records[0].first_failed_boundary.as_deref(),
            Some("scope")
        );
    }

    #[test]
    fn foreign_worktree_is_unknown_even_when_cwd_matches() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"created_by\":\"master\",\"cwd\":\"/repo\",\"worktree_path\":\"/other/playground/task-1\",\"base_commit\":\"base\",\"status\":\"working\",\"created_ms\":1,\"updated_ms\":1}}\n";
        let report = inspect_jsonl_with_options(bytes, &canonical_options());
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("WORKTREE_OUTSIDE_PROJECT_SCOPE:expected=/repo:observed=/other/playground/task-1")
        );
        assert_eq!(
            report.records[0].first_failed_boundary.as_deref(),
            Some("binding")
        );
    }

    #[test]
    fn foreign_registered_cwd_is_not_direct() {
        let bytes = b"{\"record_id\":\"worker-1\",\"sequence\":1,\"ev\":\"Registered\",\"worker\":{\"id\":\"peer\",\"token\":\"token\",\"pane\":\"%1\",\"cwd\":\"/other\",\"registered_ms\":1}}\n";
        let report = inspect_jsonl_with_options(bytes, &canonical_options());
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("CWD_OUTSIDE_PROJECT_SCOPE"));
    }

    #[test]
    fn canonical_context_does_not_fill_missing_active_task_cwd() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"created_by\":\"master\",\"worktree_path\":\"playground/task-1\",\"base_commit\":\"base\",\"status\":\"working\",\"created_ms\":1,\"updated_ms\":1}}\n";
        let report = inspect_jsonl_with_options(bytes, &canonical_options());
        assert_eq!(report.classification, MappingClass::Reset);
        assert_eq!(report.records[0].cwd, None);
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("MISSING_BINDING_EVIDENCE:cwd")
        );
        assert_eq!(
            report.records[0].first_failed_boundary.as_deref(),
            Some("binding")
        );
    }

    #[test]
    fn non_absolute_canonical_context_is_unknown() {
        let options = InspectOptions {
            source_path: None,
            canonical_project_cwd: Some(PathBuf::from("repo")),
            configured_worktree_base: None,
        };
        let report = inspect_jsonl_with_options(direct_line("one", 1).as_bytes(), &options);
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("INVALID_CANONICAL_PROJECT_CWD"));
    }

    #[test]
    fn unknown_or_missing_event_cannot_be_direct() {
        let unknown =
            inspect_jsonl(b"{\"record_id\":\"one\",\"sequence\":1,\"ev\":\"FutureEvent\"}\n");
        assert_eq!(unknown.classification, MappingClass::Unknown);
        assert_eq!(
            unknown.records[0].exact_error.as_deref(),
            Some("UNKNOWN_EVENT:FutureEvent")
        );

        let missing = inspect_jsonl(b"{\"record_id\":\"one\",\"sequence\":1}\n");
        assert_eq!(missing.classification, MappingClass::Unknown);
        assert_eq!(
            missing.records[0].exact_error.as_deref(),
            Some("EVENT_ABSENT")
        );
    }

    #[test]
    fn sent_without_message_payload_is_unknown() {
        let report = inspect_jsonl(b"{\"record_id\":\"sent-1\",\"sequence\":1,\"ev\":\"Sent\"}\n");
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(report.records[0].classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Sent:")));
        assert_eq!(
            report.records[0].first_failed_boundary.as_deref(),
            Some("schema")
        );
    }

    #[test]
    fn sent_with_non_object_message_payload_is_unknown() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"sent-1\",\"sequence\":1,\"ev\":\"Sent\",\"msg\":null}\n",
        );
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Sent:")));
    }

    #[test]
    fn sent_with_incomplete_message_payload_is_unknown() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"sent-1\",\"sequence\":1,\"ev\":\"Sent\",\"msg\":{}}\n",
        );
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Sent:")));
    }

    #[test]
    fn valid_sent_message_payload_can_be_direct() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"sent-1\",\"sequence\":1,\"ev\":\"Sent\",\"msg\":{\"id\":\"message-1\",\"from\":\"peer-a\",\"to\":\"peer-b\",\"type\":\"notify\",\"subject\":\"subject\",\"body\":\"body\",\"in_reply_to\":null,\"created_ms\":1,\"state\":\"pending\"}}\n",
        );
        assert_eq!(report.classification, MappingClass::Direct);
        assert_eq!(report.records[0].classification, MappingClass::Direct);
        assert_eq!(report.records[0].entity_id.as_deref(), Some("message-1"));
    }

    #[test]
    fn sent_without_optional_reply_reference_can_be_direct() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"sent-optional-1\",\"sequence\":1,\"ev\":\"Sent\",\"msg\":{\"id\":\"message-optional-1\",\"from\":\"peer-a\",\"to\":\"peer-b\",\"type\":\"notify\",\"subject\":\"subject\",\"body\":\"body\",\"created_ms\":1,\"state\":\"pending\"}}\n",
        );
        assert_eq!(report.classification, MappingClass::Direct);
        assert_eq!(report.records[0].classification, MappingClass::Direct);
        assert!(report.records[0].exact_error.is_none());
    }

    #[test]
    fn sent_accepts_legacy_wakeup_aliases_when_schema_is_complete() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"sent-alias-1\",\"sequence\":1,\"ev\":\"Sent\",\"msg\":{\"id\":\"message-alias-1\",\"from\":\"peer-a\",\"to\":\"peer-b\",\"type\":\"notify\",\"subject\":\"subject\",\"body\":\"body\",\"in_reply_to\":null,\"created_ms\":1,\"state\":\"pending\",\"nudge_count\":2,\"last_nudge_ms\":3}}\n",
        );
        assert_eq!(report.classification, MappingClass::Direct);
        assert!(report.records[0].exact_error.is_none());
    }

    #[test]
    fn sent_rejects_wrong_type_wakeup_count_alias() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"sent-alias-bad-count\",\"sequence\":1,\"ev\":\"Sent\",\"msg\":{\"id\":\"message-alias-bad-count\",\"from\":\"peer-a\",\"to\":\"peer-b\",\"type\":\"notify\",\"subject\":\"subject\",\"body\":\"body\",\"in_reply_to\":null,\"created_ms\":1,\"state\":\"pending\",\"nudge_count\":\"2\"}}\n",
        );
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Sent:")));
    }

    #[test]
    fn sent_rejects_wrong_type_last_wakeup_alias() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"sent-alias-bad-time\",\"sequence\":1,\"ev\":\"Sent\",\"msg\":{\"id\":\"message-alias-bad-time\",\"from\":\"peer-a\",\"to\":\"peer-b\",\"type\":\"notify\",\"subject\":\"subject\",\"body\":\"body\",\"in_reply_to\":null,\"created_ms\":1,\"state\":\"pending\",\"last_nudge_ms\":\"3\"}}\n",
        );
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Sent:")));
    }

    #[test]
    fn registered_without_worker_payload_is_unknown() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"worker-missing\",\"sequence\":1,\"ev\":\"Registered\"}\n",
        );
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(report.records[0].classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Registered:")));
        assert_eq!(
            report.records[0].first_failed_boundary.as_deref(),
            Some("schema")
        );
    }

    #[test]
    fn registered_with_wrong_worker_type_is_unknown() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"worker-wrong-type\",\"sequence\":1,\"ev\":\"Registered\",\"worker\":[] }\n",
        );
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Registered:")));
    }

    #[test]
    fn registered_with_invalid_worker_object_is_unknown() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"worker-invalid-object\",\"sequence\":1,\"ev\":\"Registered\",\"worker\":{}}\n",
        );
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Registered:")));
    }

    #[test]
    fn complete_registered_payload_can_be_direct() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"worker-valid\",\"sequence\":1,\"ev\":\"Registered\",\"worker\":{\"id\":\"peer\",\"token\":\"token\",\"pane\":\"%1\",\"cwd\":\"/repo\",\"registered_ms\":1}}\n",
        );
        assert_eq!(report.classification, MappingClass::Direct);
        assert_eq!(report.records[0].classification, MappingClass::Direct);
        assert!(report.records[0].exact_error.is_none());
    }

    #[test]
    fn delivered_without_ids_is_unknown() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"delivered-missing-ids\",\"sequence\":1,\"ev\":\"Delivered\"}\n",
        );
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Delivered:")));
    }

    #[test]
    fn acked_without_ids_is_unknown() {
        let report = inspect_jsonl(
            b"{\"record_id\":\"acked-missing-ids\",\"sequence\":1,\"ev\":\"Acked\"}\n",
        );
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.records[0]
            .exact_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_EVENT:Acked:")));
    }

    #[test]
    fn unknown_schema_failure_survives_active_binding_reset() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"FutureTaskEvent\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"status\":\"working\"}}\n";
        let report = inspect_jsonl(bytes);
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(report.records[0].classification, MappingClass::Unknown);
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("UNKNOWN_EVENT:FutureTaskEvent")
        );
        assert_eq!(
            report.records[0].first_failed_boundary.as_deref(),
            Some("schema")
        );
    }

    #[test]
    fn scope_failure_survives_active_binding_reset() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"created_by\":\"master\",\"cwd\":\"/other\",\"worktree_path\":\"playground/task-1\",\"base_commit\":\"base\",\"status\":\"working\",\"created_ms\":1,\"updated_ms\":1}}\n";
        let report = inspect_jsonl_with_options(bytes, &canonical_options());
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(report.records[0].classification, MappingClass::Unknown);
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("CWD_OUTSIDE_PROJECT_SCOPE:expected=/repo:observed=/other")
        );
        assert_eq!(
            report.records[0].first_failed_boundary.as_deref(),
            Some("scope")
        );
    }

    #[test]
    fn unknown_task_status_cannot_be_direct() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"created_by\":\"master\",\"cwd\":\"/repo\",\"worktree_path\":\"playground/task-1\",\"base_commit\":\"base\",\"status\":\"future\",\"created_ms\":1,\"updated_ms\":1}}\n";
        let report = inspect_jsonl(bytes);
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("UNKNOWN_TASK_STATUS:future")
        );
    }

    #[test]
    fn maximum_sequence_overflow_is_fail_closed() {
        let bytes = format!(
            "{}{}",
            direct_line("max-a", u64::MAX),
            direct_line("max-b", u64::MAX)
        );
        let report = inspect_jsonl(bytes.as_bytes());
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("SEQUENCE_OVERFLOW"));
        assert!(report
            .records
            .iter()
            .all(|record| record.classification == MappingClass::Unknown));
    }

    #[test]
    fn malformed_middle_preserves_valid_prefix_and_suffix() {
        let bytes = format!(
            "{}{{bad json}}\n{}",
            direct_line("one", 1),
            direct_line("two", 2)
        );
        let report = inspect_jsonl(bytes.as_bytes());
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(report.records.len(), 3);
        assert_eq!(report.records[0].line_number, 1);
        assert_eq!(report.records[1].line_number, 2);
        assert_eq!(report.records[1].raw_bytes, b"{bad json}\n");
        assert_eq!(report.records[2].line_number, 3);
        assert!(report.has_error("MALFORMED_JSON_MIDDLE"));
    }

    #[test]
    fn malformed_records_do_not_create_replay_errors() {
        let bytes = format!(
            "{}{{broken}}\n{}",
            direct_line("one", 1),
            direct_line("two", 2)
        );
        let report = inspect_jsonl(bytes.as_bytes());
        assert!(report.has_error("MALFORMED_JSON_MIDDLE"));
        assert!(!report.has_error("MISSING_SEQUENCE"));
        assert_eq!(report.records[1].classification, MappingClass::Unknown);
    }

    #[test]
    fn malformed_tail_and_missing_newline_are_explicit() {
        let bytes = format!("{}{{\"record_id\":\"tail\"", direct_line("one", 1));
        let report = inspect_jsonl(bytes.as_bytes());
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(report.records.len(), 2);
        assert_eq!(report.records[1].line_number, 2);
        assert_eq!(report.records[1].raw_bytes, b"{\"record_id\":\"tail\"");
        assert!(report.has_error("MALFORMED_JSON_TAIL"));
        assert!(report.has_error("MISSING_FINAL_NEWLINE"));
    }

    #[test]
    fn valid_json_that_is_not_an_object_is_not_a_record() {
        let report = inspect_jsonl(b"[]\n");
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].raw_bytes, b"[]\n");
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("INVALID_RECORD_SHAPE")
        );
        assert!(report.has_error("INVALID_RECORD_SHAPE"));
    }

    #[test]
    fn duplicate_record_ids_fail_closed() {
        let first = direct_line("same", 1);
        let second = direct_line("same", 2);
        let report = inspect_jsonl(format!("{first}{second}").as_bytes());
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("DUPLICATE_RECORD_ID"));
        assert!(report
            .records
            .iter()
            .all(|record| record.classification == MappingClass::Unknown));
    }

    #[test]
    fn sequence_gap_is_not_replayable() {
        let bytes = format!("{}{}", direct_line("one", 1), direct_line("three", 3));
        let report = inspect_jsonl(bytes.as_bytes());
        assert_eq!(report.classification, MappingClass::Unknown);
        assert!(report.has_error("SEQUENCE_GAP"));
        assert_eq!(report.records[1].classification, MappingClass::Unknown);
    }

    #[test]
    fn unknown_owner_is_operator_input() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"status\":\"working\",\"created_by\":\"master\",\"cwd\":\"/repo\",\"worktree_path\":\"playground/task-1\",\"base_commit\":\"base\",\"created_ms\":1,\"updated_ms\":1}}\n";
        let report = inspect_jsonl(bytes);
        assert_eq!(report.classification, MappingClass::Unknown);
        assert_eq!(report.records[0].classification, MappingClass::Unknown);
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("INVALID_EVENT:TaskUpdated:missing field `owner`")
        );
    }

    #[test]
    fn active_task_without_binding_is_reset() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"created_by\":\"master\",\"status\":\"working\",\"created_ms\":1,\"updated_ms\":1}}\n";
        let report = inspect_jsonl(bytes);
        assert_eq!(report.classification, MappingClass::Reset);
        assert_eq!(report.records[0].classification, MappingClass::Reset);
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("MISSING_BINDING_EVIDENCE:worktree,base_commit,cwd")
        );
    }

    #[test]
    fn rework_task_without_binding_is_reset() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"created_by\":\"master\",\"status\":\"rework\",\"created_ms\":1,\"updated_ms\":1}}\n";
        let report = inspect_jsonl(bytes);
        assert_eq!(report.classification, MappingClass::Reset);
        assert_eq!(report.records[0].classification, MappingClass::Reset);
        assert_eq!(
            report.records[0].exact_error.as_deref(),
            Some("MISSING_BINDING_EVIDENCE:worktree,base_commit,cwd")
        );
    }

    #[test]
    fn legacy_records_are_adapted_without_inventing_sequence() {
        let bytes = b"{\"ev\":\"Registered\",\"worker\":{\"id\":\"peer\",\"token\":\"token\",\"pane\":\"%1\",\"cwd\":\"/repo\",\"registered_ms\":1}}\n";
        let report = inspect_jsonl(bytes);
        assert_eq!(report.classification, MappingClass::Adapt);
        assert_eq!(report.records[0].classification, MappingClass::Adapt);
        assert!(report.has_error("LEGACY_SEQUENCE_ABSENT"));
    }

    #[test]
    fn duplicate_registration_requires_identity_reset() {
        let bytes = b"{\"ev\":\"Registered\",\"worker\":{\"id\":\"peer\",\"token\":\"token\",\"pane\":\"%1\",\"cwd\":\"/repo\",\"registered_ms\":1}}\n{\"ev\":\"Registered\",\"worker\":{\"id\":\"peer\",\"token\":\"token\",\"pane\":\"%1\",\"cwd\":\"/repo\",\"registered_ms\":2}}\n";
        let report = inspect_jsonl(bytes);
        assert_eq!(report.classification, MappingClass::Reset);
        assert!(report.has_error("DUPLICATE_REGISTRATION"));
        assert!(report
            .records
            .iter()
            .all(|record| record.classification == MappingClass::Reset));
    }

    #[test]
    fn digest_drift_is_reported_exactly() {
        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        let error = report
            .verify_digest("fnv1a64:0000000000000000")
            .unwrap_err();
        assert!(error
            .to_string()
            .starts_with("DIGEST_DRIFT:expected=fnv1a64:0000000000000000:observed=sha256:"));
    }

    #[test]
    fn sha256_digest_matches_known_vector() {
        assert_eq!(
            digest_bytes(b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn terminal_task_without_binding_is_historical_adapt() {
        let bytes = b"{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"created_by\":\"master\",\"status\":\"closed\",\"created_ms\":1,\"updated_ms\":1}}\n";
        let report = inspect_jsonl(bytes);
        assert_eq!(report.classification, MappingClass::Adapt);
        assert_eq!(report.records[0].classification, MappingClass::Adapt);
        assert!(report.has_error("LEGACY_BINDING_EVIDENCE_ABSENT"));
    }

    fn valid_snapshot(report: &InspectionReport) -> ImmutableSnapshot {
        ImmutableSnapshot::new(
            "migration-1",
            "project-1",
            report.source_digest.clone(),
            "archive/migration-1",
            report.source_digest.clone(),
            0,
        )
        .expect("snapshot")
    }

    fn valid_receipts(source_digest: &str, target_epoch: u64) -> MigrationReceiptSet {
        use crate::identity::{AgentId, OperationId};

        MigrationReceiptSet {
            writer: Some(
                MigrationWriterReceipt::new(
                    "migration-1",
                    "project-1",
                    None,
                    target_epoch,
                    source_digest,
                    AgentId::new("writer-1").unwrap(),
                    OperationId::new("writer-op-1").unwrap(),
                    7,
                    1,
                    1,
                )
                .unwrap(),
            ),
            identity_rebinds: Vec::new(),
            runtime_rebinds: Vec::new(),
        }
    }

    #[test]
    fn verified_prefix_preserves_the_direct_prefix_and_first_stop() {
        let bytes = format!(
            "{}{{bad json}}\n{}",
            direct_line("one", 1),
            direct_line("two", 2)
        );
        let report = inspect_jsonl(bytes.as_bytes());
        let prefix = VerifiedPrefix::from_report(&report).expect("direct prefix");

        assert_eq!(prefix.records.len(), 1);
        assert_eq!(prefix.records[0].record_id, "one");
        assert_eq!(prefix.records[0].sequence, 1);
        assert_eq!(prefix.stop_line, Some(2));
        assert!(prefix
            .stop_error
            .as_deref()
            .is_some_and(|error| error.starts_with("MALFORMED_JSON_MIDDLE:")));
        assert!(!prefix.complete);
        assert_eq!(prefix.source_digest, report.source_digest);
        assert_eq!(prefix.last_sequence(), Some(1));
    }

    #[test]
    fn first_failed_record_preserves_an_empty_prefix_and_exact_stop() {
        let bytes = b"{bad json}\n";
        let report = inspect_jsonl(bytes);
        let prefix = VerifiedPrefix::from_report(&report).expect("empty incomplete prefix");

        assert!(prefix.records.is_empty());
        assert!(!prefix.complete);
        assert_eq!(prefix.stop_line, Some(1));
        assert!(prefix
            .stop_error
            .as_deref()
            .is_some_and(|error| error.starts_with("MALFORMED_JSON_TAIL:")));

        let mut transaction = MigrationTransaction::new("migration-1", "project-1").unwrap();
        transaction.snapshot = Some(valid_snapshot(&report));
        transaction.target_epoch = Some(TargetEpoch::new(None, 2, 0).unwrap());
        transaction.verified_prefix = Some(prefix);
        transaction.receipts = Some(valid_receipts(&report.source_digest, 2));
        transaction.project_admission = ProjectAdmission::Verified;
        transaction.phase = MigrationPhase::Rebound;

        assert!(matches!(
            transaction.apply(2, 0),
            Err(MigrationContractError::PrefixBlocked {
                line: Some(1),
                exact_error,
            }) if exact_error.starts_with("MALFORMED_JSON_TAIL:")
        ));
    }

    #[test]
    fn serialized_prefix_requires_rebinding_to_exact_source_bytes() {
        let bytes = direct_line("one", 1).into_bytes();
        let report = inspect_jsonl(&bytes);
        let prefix = VerifiedPrefix::from_report(&report).expect("prefix");
        let serialized = serde_json::to_value(&prefix).expect("prefix serializes");
        assert!(serialized.get("raw_prefix_bytes").is_none());

        let mut decoded: VerifiedPrefix = serde_json::from_value(serialized).expect("prefix");
        assert!(matches!(
            decoded.validate(),
            Err(MigrationContractError::Invalid {
                field: "verified_prefix.evidence",
                ..
            })
        ));
        decoded
            .verify_against_bytes(&bytes)
            .expect("exact source bytes rebind the prefix");
        decoded.validate().expect("rebound prefix validates");

        let mut forged = serde_json::to_value(&prefix).expect("prefix serializes");
        forged["prefix_digest"] = Value::String(digest_bytes(b"forged"));
        let mut forged: VerifiedPrefix = serde_json::from_value(forged).expect("forged prefix");
        assert!(matches!(
            forged.verify_against_bytes(&bytes),
            Err(MigrationContractError::DigestMismatch {
                field: "verified_prefix.prefix_digest",
                ..
            })
        ));
    }

    #[test]
    fn reset_required_transaction_rejects_apply_before_side_effects() {
        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        let snapshot = valid_snapshot(&report);
        let prefix = VerifiedPrefix::from_report(&report).expect("prefix");
        let receipts = valid_receipts(&report.source_digest, 2);
        let mut transaction = MigrationTransaction::new("migration-1", "project-1").unwrap();
        transaction.snapshot = Some(snapshot);
        transaction.target_epoch = Some(TargetEpoch::new(None, 2, 0).unwrap());
        transaction.verified_prefix = Some(prefix);
        transaction.receipts = Some(receipts);
        transaction.project_admission = ProjectAdmission::ResetRequired;

        assert!(matches!(
            transaction.apply(2, 0),
            Err(MigrationContractError::AdmissionBlocked {
                admission: ProjectAdmission::ResetRequired
            })
        ));
    }

    #[test]
    fn migration_apply_requires_snapshot_epoch_prefix_and_receipts() {
        let mut transaction = MigrationTransaction::new("migration-1", "project-1").unwrap();
        transaction.project_admission = ProjectAdmission::Verified;
        transaction.phase = MigrationPhase::Applied;

        assert!(matches!(
            transaction.apply(2, 0),
            Err(MigrationContractError::Missing("snapshot"))
        ));
    }

    #[test]
    fn migration_apply_rejects_an_incomplete_verified_prefix() {
        let bytes = format!(
            "{}{{bad json}}\n{}",
            direct_line("one", 1),
            direct_line("two", 2)
        );
        let report = inspect_jsonl(bytes.as_bytes());
        let mut transaction = MigrationTransaction::new("migration-1", "project-1").unwrap();
        transaction.snapshot = Some(valid_snapshot(&report));
        transaction.target_epoch = Some(TargetEpoch::new(None, 2, 0).unwrap());
        transaction.verified_prefix = Some(VerifiedPrefix::from_report(&report).unwrap());
        transaction.receipts = Some(valid_receipts(&report.source_digest, 2));
        transaction.project_admission = ProjectAdmission::Verified;
        transaction.phase = MigrationPhase::Rebound;

        assert!(matches!(
            transaction.apply(2, 0),
            Err(MigrationContractError::PrefixBlocked {
                line: Some(2),
                exact_error,
            }) if exact_error.starts_with("MALFORMED_JSON_MIDDLE:")
        ));
    }

    #[test]
    fn migration_apply_requires_rebound_phase_and_returns_a_pure_receipt() {
        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        let mut transaction = MigrationTransaction::new("migration-1", "project-1").unwrap();
        transaction.snapshot = Some(valid_snapshot(&report));
        transaction.target_epoch = Some(TargetEpoch::new(None, 2, 0).unwrap());
        transaction.verified_prefix = Some(VerifiedPrefix::from_report(&report).unwrap());
        transaction.receipts = Some(valid_receipts(&report.source_digest, 2));
        transaction.project_admission = ProjectAdmission::Verified;

        assert!(matches!(
            transaction.apply(2, 0),
            Err(MigrationContractError::Invalid { field: "phase", .. })
        ));

        transaction.phase = MigrationPhase::Rebound;
        assert!(matches!(
            transaction.apply(1, 0),
            Err(MigrationContractError::EpochMismatch {
                field: "target_epoch",
                expected: 1,
                observed: 2,
            })
        ));
        let receipt = transaction.apply(2, 0).expect("complete rebound applies");
        assert_eq!(receipt.migration_id, "migration-1");
        assert_eq!(receipt.source_project_id, "project-1");
        assert_eq!(receipt.target_epoch, 2);
        assert_eq!(receipt.source_snapshot_digest, report.source_digest);
        assert_eq!(receipt.writer_operation_id, "writer-op-1");
    }

    #[test]
    fn snapshot_allows_an_independent_archive_digest() {
        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        let mut snapshot = valid_snapshot(&report);
        snapshot.archive_digest = "sha256:other-archive".into();

        snapshot
            .validate()
            .expect("archive has independent evidence digest");
        snapshot
            .verify_source_digest(&report.source_digest)
            .expect("source digest remains bound");
    }

    #[test]
    fn source_epoch_zero_is_rejected_and_global_failures_keep_their_error() {
        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        let mut snapshot = valid_snapshot(&report);
        snapshot.source_epoch = Some(0);
        assert!(matches!(
            snapshot.validate(),
            Err(MigrationContractError::Invalid {
                field: "source_epoch",
                ..
            })
        ));

        let epoch = TargetEpoch {
            source_epoch: Some(0),
            target_epoch: 2,
            expected_active_revision: 0,
        };
        assert!(matches!(
            epoch.validate(),
            Err(MigrationContractError::Invalid {
                field: "source_epoch",
                ..
            })
        ));

        let epoch = TargetEpoch::new(Some(1), 2, 7).unwrap();
        assert!(matches!(
            epoch.validate_against(1, 8),
            Err(MigrationContractError::RevisionMismatch {
                field: "expected_active_revision",
                expected: 7,
                observed: 8,
            })
        ));

        let report = inspect_jsonl_with_options(
            direct_line("one", 1).as_bytes(),
            &InspectOptions {
                canonical_project_cwd: Some(PathBuf::from("relative")),
                ..InspectOptions::default()
            },
        );
        let prefix = VerifiedPrefix::from_report(&report).expect("prefix evidence");
        assert!(!prefix.complete);
        assert_eq!(prefix.stop_line, None);
        assert!(prefix
            .stop_error
            .as_deref()
            .is_some_and(|error| error.starts_with("INVALID_CANONICAL_PROJECT_CWD:")));
        prefix.validate().expect("global stop retains valid shape");
    }

    #[test]
    fn target_epoch_requires_strictly_new_active_epoch_when_source_is_unknown() {
        for active_epoch in [3, 4] {
            let epoch = TargetEpoch::new(None, 3, 7).unwrap();
            assert!(matches!(
                epoch.validate_against(active_epoch, 7),
                Err(MigrationContractError::Invalid {
                    field: "target_epoch",
                    reason,
                }) if reason.contains("greater than active_epoch")
            ));
        }

        let epoch = TargetEpoch::new(None, 4, 7).unwrap();
        epoch
            .validate_against(3, 7)
            .expect("unknown source still advances the active epoch");
    }

    #[test]
    fn migration_transaction_binds_snapshot_and_target_source_epochs() {
        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        let snapshot = valid_snapshot(&report).with_source_epoch(Some(3));
        let mut transaction = MigrationTransaction::new("migration-1", "project-1").unwrap();
        transaction.snapshot = Some(snapshot);
        transaction.target_epoch = Some(TargetEpoch::new(None, 4, 0).unwrap());

        assert!(matches!(
            transaction.validate(),
            Err(MigrationContractError::Invalid {
                field: "target_epoch.source_epoch",
                ..
            })
        ));

        transaction.target_epoch = Some(TargetEpoch::new(Some(3), 4, 0).unwrap());
        assert!(transaction.validate().is_ok());
    }

    #[test]
    fn migration_transaction_rejects_prefix_digest_drift() {
        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        let mut snapshot = valid_snapshot(&report);
        snapshot.source_digest = "sha256:other-source".into();
        snapshot.archive_digest = snapshot.source_digest.clone();
        let prefix = VerifiedPrefix::from_report(&report).expect("prefix");
        let mut transaction = MigrationTransaction::new("migration-1", "project-1").unwrap();
        transaction.snapshot = Some(snapshot);
        transaction.target_epoch = Some(TargetEpoch::new(None, 2, 0).unwrap());
        transaction.verified_prefix = Some(prefix);
        transaction.receipts = Some(valid_receipts(&report.source_digest, 2));

        assert!(matches!(
            transaction.validate(),
            Err(MigrationContractError::DigestMismatch {
                field: "verified_prefix.source_digest",
                ..
            })
        ));
    }

    #[test]
    fn inspection_manifest_maps_each_record_and_idempotency_key() {
        let bytes = format!(
            "{}{}{}",
            direct_line("one", 1),
            "{\"record_id\":\"task-2\",\"sequence\":2,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-2\",\"owner\":\"peer\",\"created_by\":\"master\",\"status\":\"closed\",\"created_ms\":1,\"updated_ms\":1}}\n",
            "{\"record_id\":\"task-3\",\"sequence\":3,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-3\",\"owner\":\"peer\",\"created_by\":\"master\",\"status\":\"working\",\"created_ms\":1,\"updated_ms\":1}}\n"
        );
        let report = inspect_jsonl(bytes.as_bytes());
        assert_eq!(report.records.len(), 3);
        assert_eq!(report.records[0].classification, MappingClass::Direct);
        assert_eq!(report.records[1].classification, MappingClass::Adapt);
        assert_eq!(report.records[2].classification, MappingClass::Reset);

        let manifest = MigrationManifest::from_report(
            &report,
            ManifestIdentity::new("migration-1", "project-1", "/repo").unwrap(),
        )
        .expect("typed manifest");
        manifest.validate().expect("manifest validates");

        assert_eq!(manifest.source_snapshot_digest, report.source_digest);
        assert_eq!(manifest.records.len(), 3);
        assert_eq!(manifest.records[0].source_record_id, "one");
        assert_eq!(manifest.records[0].source_record_type, "TaskUpdated");
        assert_eq!(manifest.records[0].mapping_class, MappingClass::Direct);
        assert_eq!(
            manifest.records[0].source_disposition,
            SourceDisposition::DirectReplay
        );
        assert_eq!(manifest.records[1].mapping_class, MappingClass::Adapt);
        assert_eq!(
            manifest.records[1].source_disposition,
            SourceDisposition::AdaptReconcile
        );
        assert_eq!(manifest.records[2].mapping_class, MappingClass::Reset);
        assert_eq!(
            manifest.records[2].source_disposition,
            SourceDisposition::RebuildRequired
        );
        assert_eq!(
            manifest.records[2].blocker_code.as_deref(),
            Some("MISSING_BINDING_EVIDENCE")
        );
        assert_eq!(
            manifest.records[2].first_failed_boundary.as_deref(),
            Some("binding")
        );
        assert_eq!(manifest.mapping_status, MigrationMappingStatus::Planned);
        assert_eq!(manifest.project_admission, ProjectAdmission::ResetRequired);
        assert_eq!(
            manifest.blocker_code.as_deref(),
            Some("MISSING_BINDING_EVIDENCE")
        );
        assert_eq!(manifest.first_failed_boundary.as_deref(), Some("binding"));
        assert_eq!(
            manifest.record_idempotency_key(&manifest.records[0]),
            format!(
                "project-1:one:{}:2",
                manifest.records[0].source_record_digest
            )
        );

        let serialized = serde_json::to_value(&manifest).expect("manifest serializes");
        assert!(serialized.get("raw_bytes").is_none());
        assert!(serialized["records"][0]["target_epoch"].as_str().is_some());
        assert!(serialized["records"][0]["owner_authority"]
            .as_str()
            .unwrap()
            .starts_with("inspection:"));
    }

    #[test]
    fn no_write_rehearsal_replays_verified_prefix_and_rebuilds_projection() {
        let bytes = format!("{}{}", direct_line("one", 1), direct_line("two", 2));
        let report = inspect_jsonl(bytes.as_bytes());
        let prefix = VerifiedPrefix::from_report(&report).expect("prefix");
        assert!(prefix.complete);

        let rehearsal = MigrationRehearsal::no_write(
            &report,
            &prefix,
            ManifestIdentity::new("migration-1", "project-1", "/repo").unwrap(),
        )
        .expect("rehearsal");
        assert_eq!(rehearsal.replayed_records, 2);
        assert_eq!(
            rehearsal.rebuilt_projection_tasks,
            vec!["task-one", "task-two"]
        );
        assert_eq!(rehearsal.verified_prefix_digest, prefix.prefix_digest);
        assert_eq!(
            rehearsal.projection_digest,
            digest_bytes(&rehearsal.projection_bytes)
        );
        assert!(rehearsal.rollback_fence.validate_against(1, 0).is_ok());
        assert_eq!(rehearsal.stop_error, "VERIFIED_PREFIX_COMPLETE");
    }

    #[test]
    fn direct_inspection_alone_cannot_verify_project_admission() {
        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        assert_eq!(report.classification, MappingClass::Direct);

        let manifest = MigrationManifest::from_report(
            &report,
            ManifestIdentity::new("migration-1", "project-1", "/repo").unwrap(),
        )
        .expect("inspection manifest");

        assert_eq!(manifest.project_admission, ProjectAdmission::NeedsOperator);
        assert_eq!(manifest.archive_ref, None);
        assert_eq!(manifest.archive_digest, None);
        assert_eq!(
            manifest.blocker_code.as_deref(),
            Some("PROJECT_ADMISSION_EVIDENCE_ABSENT")
        );
    }

    #[test]
    fn verified_admission_requires_archive_and_explicit_source_identity() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../docs/migration-v1-history-manifest.schema.json"
        ))
        .expect("declared manifest schema parses");
        assert!(schema["properties"]["admission_evidence"].is_object());
        assert!(schema["allOf"]
            .as_array()
            .is_some_and(
                |clauses| clauses.iter().any(|clause| clause["if"]["properties"]
                    ["project_admission"]["const"]
                    == "verified"
                    && clause["then"]["required"]
                        == serde_json::json!([
                            "archive_ref",
                            "archive_digest",
                            "admission_evidence"
                        ]))
            ));

        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        let identity = ManifestIdentity {
            source_repo: Some("/repo".to_owned()),
            source_branch: Some("main".to_owned()),
            source_head: Some("abc123".to_owned()),
            source_tree: Some("tree123".to_owned()),
            ..ManifestIdentity::new("migration-1", "project-1", "/repo").unwrap()
        };
        let admission = MigrationAdmissionEvidence {
            writer_frozen: true,
            live_scope_verified: true,
            current_source_verified: true,
            source_identity_verified: true,
            archive_ref: "archive/migration-1".to_owned(),
            archive_digest: "sha256:archive".to_owned(),
        };

        let manifest =
            MigrationManifest::from_report_with_admission(&report, identity, Some(admission))
                .expect("verified manifest");

        assert_eq!(manifest.project_admission, ProjectAdmission::Verified);
        assert_eq!(manifest.archive_ref.as_deref(), Some("archive/migration-1"));
        assert_eq!(manifest.archive_digest.as_deref(), Some("sha256:archive"));
        let serialized = serde_json::to_value(&manifest).expect("verified manifest serializes");
        assert_eq!(
            serialized["admission_evidence"]["archive_ref"].as_str(),
            Some("archive/migration-1")
        );
        assert_eq!(
            serialized["admission_evidence"]["writer_frozen"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn rehearsal_fails_instead_of_emitting_projection_after_reducer_rejection() {
        let rejected = Event::CommandRecorded {
            command_id: String::new(),
            receipt: crate::server::state::CommandReceipt {
                operation_id: "operation-1".to_owned(),
                outcome: serde_json::json!({}),
                sequence: 1,
                revision: 1,
            },
        };
        let line = canonical_event_bytes(rejected, Some("command-1"), Some(1), true);
        let report = inspect_jsonl(&line);
        assert_eq!(report.classification, MappingClass::Direct);
        let prefix = VerifiedPrefix::from_report(&report).expect("direct prefix");

        let error = MigrationRehearsal::no_write(
            &report,
            &prefix,
            ManifestIdentity::new("migration-1", "project-1", "/repo").unwrap(),
        )
        .expect_err("reducer rejection must stop rehearsal");

        assert!(matches!(
            error,
            MigrationContractError::Invalid {
                field: "rehearsal.reducer",
                ..
            }
        ));
    }

    #[test]
    fn manifest_rejects_mapped_record_with_foreign_target_epoch() {
        let report = inspect_jsonl(direct_line("one", 1).as_bytes());
        let mut manifest = MigrationManifest::from_report(
            &report,
            ManifestIdentity::new("migration-1", "project-1", "/repo").unwrap(),
        )
        .expect("manifest");
        manifest.records[0].mapping_status = ManifestRecordStatus::Mapped;
        manifest.records[0].target_sequence = Some(1);
        manifest.records[0].target_entity_id = Some("task-one".to_owned());
        manifest.records[0].target_epoch = "3".to_owned();

        assert!(matches!(
            manifest.validate(),
            Err(MigrationContractError::EpochMismatch {
                field: "manifest.record.target_epoch",
                expected: 2,
                observed: 3,
            })
        ));
    }

    #[test]
    fn needs_operator_manifest_keeps_top_level_admission_evidence() {
        let bytes = format!(
            "{}{}",
            "{\"record_id\":\"task-1\",\"sequence\":1,\"ev\":\"TaskUpdated\",\"task\":{\"id\":\"task-1\",\"owner\":\"peer\",\"created_by\":\"master\",\"status\":\"closed\",\"created_ms\":1,\"updated_ms\":1}}\n",
            direct_line("task-2", 2)
        );
        let report = inspect_jsonl(bytes.as_bytes());
        let manifest = MigrationManifest::from_report(
            &report,
            ManifestIdentity::new("migration-1", "project-1", "/repo").unwrap(),
        )
        .expect("typed manifest");

        assert_eq!(manifest.project_admission, ProjectAdmission::NeedsOperator);
        assert_eq!(
            manifest.blocker_code.as_deref(),
            Some("LEGACY_BINDING_EVIDENCE_ABSENT")
        );
        assert_eq!(manifest.first_failed_boundary.as_deref(), Some("binding"));
    }
