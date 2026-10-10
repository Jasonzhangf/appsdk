impl Server {
    /// Apply events to memory and persist them atomically-ordered in the journal.
    ///
    /// Entry point for paths that have no client to answer. A rejected batch is
    /// reported through the explicit failure channel instead of panicking while
    /// the state guard is held; callers that can answer a client use the
    /// checked entry points and propagate the error.
    pub(crate) fn commit(&self, evs: &[Event]) {
        let mut st = self.state.lock().unwrap();
        self.commit_locked_reporting(&mut st, evs);
    }

    /// Fallible reducer entry point used by typed producers.
    pub fn commit_checked(
        &self,
        evs: &[Event],
    ) -> Result<notification_contract::CommitReceipt, notification_contract::JournalError> {
        let mut st = self.state.lock().unwrap();
        self.commit_locked_checked(&mut st, evs)
    }

    /// Compatibility entry point for callers that only need a string error.
    /// The checked reducer remains the single journal/state owner.
    pub(crate) fn try_commit(&self, evs: &[Event]) -> Result<(), String> {
        self.commit_checked(evs)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Compatibility entry point for callers holding the state lock.
    /// This delegates to the typed reducer and never applies state after a
    /// journal failure.
    pub(crate) fn try_commit_locked(&self, st: &mut State, evs: &[Event]) -> Result<(), String> {
        self.commit_locked_checked(st, evs)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Commit one command and its outcome atomically. A retry with the same
    /// command id returns the recorded outcome without appending another event.
    /// Reusing a command id for a different operation is rejected explicitly.
    pub fn commit_command(
        &self,
        command_id: &str,
        operation_id: &str,
        evs: &[Event],
        outcome: serde_json::Value,
    ) -> Result<notification_contract::CommandOutcome, notification_contract::JournalError> {
        validate_command_id(command_id)?;
        validate_command_id(operation_id)?;
        let mut st = self.state.lock().unwrap();
        self.commit_command_locked(&mut st, command_id, operation_id, evs, outcome, None, None)
    }

    fn commit_command_at_revision(
        &self,
        command_id: &str,
        operation_id: &str,
        evs: &[Event],
        outcome: serde_json::Value,
        expected_revision: u64,
    ) -> Result<notification_contract::CommandOutcome, notification_contract::JournalError> {
        validate_command_id(command_id)?;
        validate_command_id(operation_id)?;
        let mut st = self.state.lock().unwrap();
        self.commit_command_locked(
            &mut st,
            command_id,
            operation_id,
            evs,
            outcome,
            Some(expected_revision),
            None,
        )
    }

    fn commit_command_locked(
        &self,
        st: &mut State,
        command_id: &str,
        operation_id: &str,
        evs: &[Event],
        outcome: serde_json::Value,
        expected_revision: Option<u64>,
        outer_binding: Option<&RegisterOuterBinding>,
    ) -> Result<notification_contract::CommandOutcome, notification_contract::JournalError> {
        let typed_command_id = CommandId::new(command_id.to_owned()).map_err(|error| {
            notification_contract::JournalError::InvalidCommand(error.to_string())
        })?;
        if let Some(existing) = st.global.lookup_command_receipt(&typed_command_id) {
            if existing.operation_id.as_str() != operation_id {
                return Err(notification_contract::JournalError::InvalidCommand(
                    format!(
                        "command_id {command_id} already belongs to operation {}",
                        existing.operation_id
                    ),
                ));
            }
            // Same-peer context reuse can name this already-completed Register
            // transaction from a new outer operation. The durable receipt is
            // the owner evidence for that exact nested ID pair; bind it to the
            // new outer operation without writing or replaying Register.
            let scheduled_binding = take_context_register_start_binding();
            let start_binding = outer_binding.or(scheduled_binding.as_ref());
            take_context_cancel_operation();
            if let Some(binding) = start_binding {
                let expected = (command_id.to_owned(), operation_id.to_owned());
                if binding.bound().as_ref() != Some(&expected) {
                    return Err(notification_contract::JournalError::InvalidCommand(
                        "IDENTITY_OPERATION_NESTED_RECEIPT_MISMATCH: committed Register receipt does not match the outer operation binding".into(),
                    ));
                }
                binding
                    .promote_inner_dispatched()
                    .map_err(notification_contract::JournalError::InvalidCommand)?;
            }
            return Ok(notification_contract::CommandOutcome {
                receipt: notification_contract::CommitReceipt {
                    sequence: existing.sequence,
                    revision: existing.revision,
                },
                operation_id: existing.operation_id.as_str().to_owned(),
                outcome: existing.outcome.clone(),
                replayed: true,
            });
        }
        if let Some(existing) = st.command_receipts.get(command_id) {
            if existing.operation_id != operation_id {
                return Err(notification_contract::JournalError::InvalidCommand(
                    format!(
                        "command_id {command_id} already belongs to operation {}",
                        existing.operation_id
                    ),
                ));
            }
            return Ok(notification_contract::CommandOutcome {
                receipt: notification_contract::CommitReceipt {
                    sequence: existing.sequence,
                    revision: existing.revision,
                },
                operation_id: existing.operation_id.clone(),
                outcome: existing.outcome.clone(),
                replayed: true,
            });
        }
        if let Some((existing_command_id, _)) =
            st.global
                .command_receipts
                .iter()
                .find(|(existing_command_id, receipt)| {
                    *existing_command_id != command_id
                        && receipt.operation_id.as_str() == operation_id
                })
        {
            return Err(notification_contract::JournalError::InvalidCommand(
                format!(
                    "operation_id {operation_id} already belongs to command_id {existing_command_id}"
                ),
            ));
        }
        if let Some((existing_command_id, _)) =
            st.command_receipts
                .iter()
                .find(|(existing_command_id, receipt)| {
                    *existing_command_id != command_id && receipt.operation_id == operation_id
                })
        {
            return Err(notification_contract::JournalError::InvalidCommand(
                format!(
                    "operation_id {operation_id} already belongs to command_id {existing_command_id}"
                ),
            ));
        }
        state::responsibility_preflight(st, evs)
            .map_err(notification_contract::JournalError::InvalidCommand)?;
        if let Some(expected_revision) = expected_revision {
            let observed_revision = st.revision;
            if observed_revision != expected_revision {
                return Err(notification_contract::JournalError::InvalidCommand(format!(
                    "compare-and-swap revision mismatch: expected {expected_revision}, observed {observed_revision}"
                )));
            }
        }
        let event_count = evs.len().checked_add(2).ok_or_else(|| {
            notification_contract::JournalError::InvalidCommand(
                "command event count overflow".into(),
            )
        })? as u64;
        let sequence = st.sequence.checked_add(event_count).ok_or_else(|| {
            notification_contract::JournalError::InvalidCommand("sequence counter overflow".into())
        })?;
        let revision = st.revision.checked_add(event_count).ok_or_else(|| {
            notification_contract::JournalError::InvalidCommand("revision counter overflow".into())
        })?;
        let receipt = state::CommandReceipt {
            operation_id: operation_id.to_owned(),
            outcome: outcome.clone(),
            sequence,
            revision,
        };
        let started = Event::CommandStarted {
            command_id: command_id.to_owned(),
            operation_id: operation_id.to_owned(),
        };
        let completed = Event::CommandCompleted {
            command_id: command_id.to_owned(),
            operation_id: operation_id.to_owned(),
            receipt: receipt.clone(),
        };
        self.append_command_phase_locked(
            st,
            std::slice::from_ref(&started),
            CommandJournalPhase::Start,
        )?;
        let scheduled_binding = take_context_register_start_binding();
        let start_binding = outer_binding.or(scheduled_binding.as_ref());
        #[cfg(feature = "context-cancel-test-hooks")]
        if let Some(context_operation_id) = take_context_cancel_operation() {
            let reply = crate::context_cancel_test_hooks::barrier_reply(
                "register_start_synced",
                &context_operation_id,
                std::process::id(),
                Some(command_id),
                Some(operation_id),
                || false,
            );
            if reply.as_deref() == Some("fail_promotion") {
                crate::server::operation_journal::inject_next_append_fault();
            }
        }
        if let Some(binding) = start_binding {
            binding
                .promote_inner_dispatched()
                .map_err(notification_contract::JournalError::InvalidCommand)?;
        }
        self.append_command_phase_locked(st, evs, CommandJournalPhase::Business)?;
        self.append_command_phase_locked(
            st,
            std::slice::from_ref(&completed),
            CommandJournalPhase::Completion,
        )?;
        let mut events = Vec::with_capacity(evs.len() + 2);
        events.push(started);
        events.extend_from_slice(evs);
        events.push(completed);
        self.apply_committed_events(st, &events)?;
        let has_pending_scheduler_admission = events.iter().any(|event| {
            matches!(
                event,
                Event::SchedulerAdmission { admission } if admission.status == "pending"
            )
        });
        let has_succeeded_scheduler_admission = events.iter().any(|event| {
            let Event::SchedulerAdmissionStatus {
                request_id, status, ..
            } = event
            else {
                return false;
            };
            status == "succeeded"
                && st
                    .scheduler_admissions
                    .get(request_id)
                    .is_some_and(|admission| {
                        admission.status == "succeeded"
                            && st.msgs.get(&admission.message_id).is_some_and(|message| {
                                message.state == "pending"
                                    && st.scheduler_message_deliverable(&message.id)
                            })
                    })
        });
        if (events
            .iter()
            .any(|event| matches!(event, Event::Sent { .. }))
            && !has_pending_scheduler_admission)
            || has_succeeded_scheduler_admission
            || events
                .iter()
                .any(|event| matches!(event, Event::GlobalRuntimeBound { .. }))
        {
            self.mailbox_notify.notify_waiters();
        }
        Ok(notification_contract::CommandOutcome {
            receipt: notification_contract::CommitReceipt { sequence, revision },
            operation_id: operation_id.to_owned(),
            outcome,
            replayed: false,
        })
    }

    /// Commit events while the caller holds the state guard.
    ///
    /// A rejected batch is returned to the caller as an error. The journal
    /// owner has already rolled the batch back and poisoned this owner, so
    /// this never panics while the caller holds the process-wide state mutex.
    pub(crate) fn commit_locked(
        &self,
        st: &mut State,
        evs: &[Event],
    ) -> Result<(), notification_contract::JournalError> {
        self.commit_locked_checked(st, evs).map(|_| ())
    }

    /// Commit events while the caller holds the state guard, for callers that
    /// have no client to answer.
    ///
    /// The journal owner is already fail-closed after a rejection, so the
    /// failure is reported through the explicit failure channel and the daemon
    /// keeps serving reads instead of panicking while the state guard is held.
    /// Callers that can answer a client use `commit_locked` and propagate.
    pub(crate) fn commit_locked_reporting(&self, st: &mut State, evs: &[Event]) {
        if let Err(error) = self.commit_locked(st, evs) {
            self.report_journal_commit_error(&error);
        }
    }

    pub(crate) fn commit_locked_checked(
        &self,
        st: &mut State,
        evs: &[Event],
    ) -> Result<notification_contract::CommitReceipt, notification_contract::JournalError> {
        if let Some(error) = &st.journal_poison {
            return Err(notification_contract::JournalError::Append(error.clone()));
        }
        state::responsibility_preflight(st, evs)
            .map_err(notification_contract::JournalError::InvalidCommand)?;
        use std::io::Write;
        // Persist control truth before any state change or external notification.
        // A failed journal poisons this owner instead of silently resetting budgets.
        let mut buf = Vec::new();
        for ev in evs {
            let line = match serde_json::to_string(ev) {
                Ok(line) => line,
                Err(error) => {
                    let message = error.to_string();
                    st.journal_poison = Some(message.clone());
                    return Err(notification_contract::JournalError::Append(message));
                }
            };
            buf.extend_from_slice(line.as_bytes());
            buf.push(b'\n');
        }
        #[cfg(test)]
        let append_fault = SUBAGENT_JOURNAL_FAULT.with(|injected| {
            let injected_fault = injected.get();
            let close_final = injected_fault == SubagentJournalFault::CloseFinalAppend as u8
                && evs.iter().any(|event| {
                matches!(event, Event::SubagentUpdated { subagent } if subagent.status == "closed")
            });
            if close_final
                || injected_fault == SubagentJournalFault::StartAppend as u8
                || injected_fault == SubagentJournalFault::CloseFirstAppend as u8
                || injected_fault == SubagentJournalFault::WorkingAppend as u8
            {
                injected.set(0);
                true
            } else {
                false
            }
        });
        #[cfg(not(test))]
        let append_fault = false;
        #[cfg(feature = "context-cancel-test-hooks")]
        let peer_lifecycle_completion_fault = std::env::var(
            "COLLAB_TEST_FAIL_PEER_LIFECYCLE_COMPLETION_APPEND",
        )
        .ok()
        .is_some_and(|operation_id| {
            evs.iter().any(|event| {
                matches!(event, Event::PeerLifecycleOperationRecorded { operation }
                    if operation.operation_id == operation_id
                        && operation.phase == crate::proto::PeerLifecyclePhase::Complete)
            })
        });
        #[cfg(not(feature = "context-cancel-test-hooks"))]
        let peer_lifecycle_completion_fault = false;
        #[cfg(test)]
        let (task_register_append_fault, task_register_sync_fault) = TASK_REGISTER_JOURNAL_FAULT
            .with(|injected| {
                let injected_fault = injected.get();
                let has_task_registration = evs
                    .iter()
                    .any(|event| matches!(event, Event::TaskCreated { .. }));
                if !has_task_registration {
                    return (false, false);
                }
                if injected_fault == TaskRegisterJournalFault::Append as u8 {
                    injected.set(0);
                    (true, false)
                } else if injected_fault == TaskRegisterJournalFault::Sync as u8 {
                    injected.set(0);
                    (false, true)
                } else {
                    (false, false)
                }
            });
        #[cfg(not(test))]
        let (task_register_append_fault, task_register_sync_fault) = (false, false);
        #[cfg(test)]
        let current_thread_route_journal_fault =
            CURRENT_THREAD_ROUTE_JOURNAL_FAULT.with(|injected| {
                let has_current_thread_route = evs
                    .iter()
                    .any(|event| matches!(event, Event::GlobalCurrentThreadRouteSet { .. }));
                if has_current_thread_route {
                    injected.replace(None)
                } else {
                    None
                }
            });
        #[cfg(not(test))]
        let current_thread_route_journal_fault: Option<()> = None;
        if append_fault {
            let message = "injected subagent journal append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        if peer_lifecycle_completion_fault {
            let message = "injected peer lifecycle completion append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        #[cfg(test)]
        if matches!(
            current_thread_route_journal_fault,
            Some(CurrentThreadRouteJournalFault::Append)
        ) {
            let message = "injected current thread route journal append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        if task_register_append_fault {
            let message = "injected task register journal append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        let mut j = self.journal.lock().unwrap();
        // Record the durable length before the append so a batch the reducer
        // rejects can be rolled back instead of being replayed as an
        // unreducible event after a restart.
        let journal_len = match j.metadata() {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                let message = error.to_string();
                st.journal_poison = Some(message.clone());
                return Err(notification_contract::JournalError::Append(message));
            }
        };
        if let Err(error) = j.write_all(&buf) {
            let message = error.to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        #[cfg(test)]
        let sync_fault = SUBAGENT_JOURNAL_FAULT.with(|injected| {
            let injected_fault = injected.get();
            let close_final = injected_fault == SubagentJournalFault::CloseFinalSync as u8
                && evs.iter().any(|event| {
                matches!(event, Event::SubagentUpdated { subagent } if subagent.status == "closed")
            });
            if close_final
                || injected_fault == SubagentJournalFault::StartSync as u8
                || injected_fault == SubagentJournalFault::CloseFirstSync as u8
                || injected_fault == SubagentJournalFault::WorkingSync as u8
            {
                injected.set(0);
                true
            } else {
                false
            }
        });
        #[cfg(not(test))]
        let sync_fault = false;
        if sync_fault {
            let message = "injected subagent journal sync failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        #[cfg(test)]
        if matches!(
            current_thread_route_journal_fault,
            Some(CurrentThreadRouteJournalFault::Sync)
        ) {
            let message = "injected current thread route journal sync failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        if task_register_sync_fault {
            let message = "injected task register journal sync failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        if let Err(error) = j.sync_data() {
            let message = error.to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        if let Err(error) = self.apply_committed_events(st, evs) {
            // The append is already durable, but the reducer rejected the
            // batch. Roll the journal back to its pre-append length and re-sync
            // so a rejected event is never replayed: without this, the bad line
            // is permanent and even a restart cannot reduce the journal.
            //
            // The owner stays poisoned. `apply_committed_events` applies events
            // one by one, so the events before the rejected one are already in
            // memory and absent from the rolled-back journal. Clearing the
            // poison here would let later commits succeed on a state that no
            // longer matches the journal. Poison keeps writes fail-closed, and
            // because the poison is in-memory only, replaying the repaired
            // journal restores a clean owner.
            if let Err(rollback_error) = j.set_len(journal_len).and_then(|()| j.sync_data()) {
                st.journal_poison = Some(format!(
                    "journal rollback failed after reducer rejection: {rollback_error}"
                ));
            }
            return Err(error);
        }
        let has_pending_scheduler_admission = evs.iter().any(|event| {
            matches!(
                event,
                Event::SchedulerAdmission { admission } if admission.status == "pending"
            )
        });
        let has_succeeded_scheduler_admission = evs.iter().any(|event| {
            let Event::SchedulerAdmissionStatus {
                request_id, status, ..
            } = event
            else {
                return false;
            };
            status == "succeeded"
                && st
                    .scheduler_admissions
                    .get(request_id)
                    .is_some_and(|admission| {
                        admission.status == "succeeded"
                            && st.msgs.get(&admission.message_id).is_some_and(|message| {
                                message.state == "pending"
                                    && st.scheduler_message_deliverable(&message.id)
                            })
                    })
        });
        if (evs.iter().any(|event| matches!(event, Event::Sent { .. }))
            && !has_pending_scheduler_admission)
            || has_succeeded_scheduler_admission
            || evs
                .iter()
                .any(|event| matches!(event, Event::GlobalRuntimeBound { .. }))
        {
            self.mailbox_notify.notify_waiters();
        }
        Ok(notification_contract::CommitReceipt {
            sequence: st.sequence,
            revision: st.revision,
        })
    }

    fn append_command_phase_locked(
        &self,
        st: &mut State,
        evs: &[Event],
        phase: CommandJournalPhase,
    ) -> Result<(), notification_contract::JournalError> {
        if let Some(error) = &st.journal_poison {
            return Err(notification_contract::JournalError::Append(error.clone()));
        }
        let mut body = Vec::new();
        for ev in evs {
            let line = match serde_json::to_string(ev) {
                Ok(line) => line,
                Err(error) => {
                    let message = error.to_string();
                    st.journal_poison = Some(message.clone());
                    return Err(notification_contract::JournalError::Append(message));
                }
            };
            body.extend_from_slice(line.as_bytes());
            body.push(b'\n');
        }
        let mut journal = self.journal.lock().unwrap();
        #[cfg(test)]
        let append_fault = COMMAND_JOURNAL_FAULT.with(|injected| {
            let expected = match phase {
                CommandJournalPhase::Start => CommandJournalFault::StartAppend as u8,
                CommandJournalPhase::Completion => CommandJournalFault::CompletionAppend as u8,
                CommandJournalPhase::Business => 0,
            };
            if injected.get() == expected && expected != 0 {
                injected.set(0);
                true
            } else {
                false
            }
        });
        #[cfg(not(test))]
        let append_fault = false;
        #[cfg(not(test))]
        let _ = phase;
        if append_fault {
            let message = "injected command journal append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        if let Err(error) = std::io::Write::write_all(&mut *journal, &body) {
            let message = error.to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        #[cfg(test)]
        let sync_fault = COMMAND_JOURNAL_FAULT.with(|injected| {
            let expected = match phase {
                CommandJournalPhase::Start => CommandJournalFault::StartSync as u8,
                CommandJournalPhase::Completion => CommandJournalFault::CompletionSync as u8,
                CommandJournalPhase::Business => 0,
            };
            if injected.get() == expected && expected != 0 {
                injected.set(0);
                true
            } else {
                false
            }
        });
        #[cfg(not(test))]
        let sync_fault = false;
        if sync_fault {
            let message = "injected command journal sync failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        if let Err(error) = journal.sync_data() {
            let message = error.to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        Ok(())
    }

    fn apply_committed_events(
        &self,
        st: &mut State,
        evs: &[Event],
    ) -> Result<(), notification_contract::JournalError> {
        for ev in evs {
            if let Err(error) = st.apply_checked(ev) {
                st.journal_poison.get_or_insert(error.clone());
                return Err(notification_contract::JournalError::Reducer(error));
            }
            if let Err(error) = st.advance_version() {
                st.journal_poison.get_or_insert(error.clone());
                return Err(notification_contract::JournalError::Reducer(error));
            }
            if let Event::Sent { msg } = ev {
                if let Err(error) = self.backup_message(msg) {
                    self.report_mailbox_projection_error(error);
                }
            }
            if let Event::Delivered { ids } = ev {
                for id in ids {
                    if let Some(msg) = st.msgs.get(id) {
                        if let Err(error) = self.backup_message(msg) {
                            self.report_mailbox_projection_error(error);
                        }
                    }
                }
            }
            if let Event::Acked { ids } = ev {
                for id in ids {
                    if let Some(msg) = st.msgs.get(id) {
                        if let Err(error) = self.backup_message(msg) {
                            self.report_mailbox_projection_error(error);
                        }
                    }
                }
            }
        }
        if let Err(error) = st.global.validate() {
            let error = format!("global reducer validation failed: {error}");
            st.journal_poison.get_or_insert(error.clone());
            return Err(notification_contract::JournalError::Reducer(error));
        }
        Ok(())
    }

    fn report_mailbox_projection_error(&self, error: String) {
        // Journal truth is already durable. Keep projection failure explicit
        // and queryable without turning it into a false delivery result.
        append_log(
            &self.log_path(),
            &format!("MAILBOX_JSONL_WRITE_FAILED: {error}"),
        );
        if let Err(activity_error) = record_activity(
            &self.storage_root,
            "mailbox_projection_error",
            json!({"exact_error": error, "recoverable": true}),
        ) {
            append_log(
                &self.log_path(),
                &format!("MAILBOX_PROJECTION_ERROR_RECORD_FAILED: {activity_error}"),
            );
        }
    }

    /// Report a rejected reducer commit from a context that has no client to
    /// answer. The journal owner has already rolled the batch back, or set
    /// `journal_poison` when the rollback failed. This keeps the failure
    /// explicit and queryable without panicking while the caller holds the
    /// state guard.
    pub(crate) fn report_journal_commit_error(&self, error: &notification_contract::JournalError) {
        append_log(&self.log_path(), &format!("JOURNAL_COMMIT_FAILED: {error}"));
        if let Err(activity_error) = record_activity(
            &self.storage_root,
            "journal_commit_error",
            json!({"exact_error": error.to_string(), "recoverable": true}),
        ) {
            append_log(
                &self.log_path(),
                &format!("JOURNAL_COMMIT_ERROR_RECORD_FAILED: {activity_error}"),
            );
        }
    }

    fn backup_message(&self, msg: &Message) -> Result<(), String> {
        mailbox::backup_message(&self.storage_root, msg)
    }

    fn rewrite_journal_locked(
        &self,
        st: &State,
    ) -> Result<(), notification_contract::JournalError> {
        let path = self.journal_path.clone();
        let tmp = path.with_file_name("journal.jsonl.tmp");
        let mut body = String::new();
        let (sequence, revision, events) = st.snapshot_contents();
        let line = serde_json::to_string(&Event::ReducerSnapshot {
            sequence,
            revision,
            events,
        })
        .map_err(|error| {
            notification_contract::JournalError::Append(format!("compact serialize: {error}"))
        })?;
        body.push_str(&line);
        body.push('\n');
        std::fs::write(&tmp, body).map_err(|error| {
            notification_contract::JournalError::Append(format!("compact write: {error}"))
        })?;
        std::fs::rename(&tmp, &path).map_err(|error| {
            notification_contract::JournalError::Append(format!("compact rename: {error}"))
        })?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                notification_contract::JournalError::Append(format!("compact reopen: {error}"))
            })?;
        *self.journal.lock().unwrap() = file;
        Ok(())
    }

}
