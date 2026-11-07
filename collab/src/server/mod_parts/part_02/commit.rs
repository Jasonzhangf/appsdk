impl Server {
    pub(crate) fn commit(&self, changes: &[Event]) {
        let mut state = self.state.lock().unwrap();
        self.commit_locked_reporting(&mut state, changes);
    }

    pub fn commit_checked(
        &self,
        changes: &[Event],
    ) -> Result<notification_contract::CommitReceipt, notification_contract::MutationError> {
        self.commit_locked_checked(&mut self.state.lock().unwrap(), changes)
    }

    pub(crate) fn try_commit(&self, changes: &[Event]) -> Result<(), String> {
        self.commit_checked(changes)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn try_commit_locked(
        &self,
        state: &mut State,
        changes: &[Event],
    ) -> Result<(), String> {
        self.commit_locked_checked(state, changes)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn commit_locked(&self, state: &mut State, changes: &[Event]) -> Result<(), String> {
        self.try_commit_locked(state, changes)
    }

    pub(crate) fn commit_locked_reporting(&self, state: &mut State, changes: &[Event]) {
        if let Err(error) = self.commit_locked_checked(state, changes) {
            self.report_state_error(&error);
        }
    }

    pub(crate) fn commit_locked_checked(
        &self,
        state: &mut State,
        changes: &[Event],
    ) -> Result<notification_contract::CommitReceipt, notification_contract::MutationError> {
        use notification_contract::MutationError;
        state::responsibility_preflight(state, changes).map_err(MutationError::InvalidCommand)?;
        if changes.is_empty() {
            return Ok(notification_contract::CommitReceipt {
                sequence: state.sequence,
                revision: state.revision,
            });
        }
        let mut candidate = state.clone();
        for change in changes {
            candidate
                .apply_checked(change)
                .map_err(MutationError::InvalidState)?;
        }
        candidate
            .advance_version()
            .map_err(MutationError::InvalidState)?;
        self.publish_current_state(state, candidate)
    }

    pub fn commit_command(
        &self,
        command: &str,
        operation: &str,
        changes: &[Event],
        outcome: serde_json::Value,
    ) -> Result<notification_contract::CommandOutcome, notification_contract::MutationError> {
        self.commit_command_locked(
            &mut self.state.lock().unwrap(),
            command,
            operation,
            changes,
            outcome,
            None,
        )
    }

    fn commit_command_locked(
        &self,
        state: &mut State,
        command: &str,
        operation: &str,
        changes: &[Event],
        outcome: serde_json::Value,
        expected_revision: Option<u64>,
    ) -> Result<notification_contract::CommandOutcome, notification_contract::MutationError> {
        use notification_contract::MutationError;
        let command_id = CommandId::new(command.to_owned())
            .map_err(|error| MutationError::InvalidCommand(error.to_string()))?;
        let operation_id = OperationId::new(operation.to_owned())
            .map_err(|error| MutationError::InvalidCommand(error.to_string()))?;
        if let Some(receipt) = state.global.lookup_command_receipt(&command_id) {
            if receipt.operation_id != operation_id {
                return Err(MutationError::InvalidCommand(format!(
                    "command {command} already belongs to another operation"
                )));
            }
            return Ok(notification_contract::CommandOutcome {
                receipt: notification_contract::CommitReceipt {
                    sequence: receipt.sequence,
                    revision: receipt.revision,
                },
                operation_id: operation.into(),
                outcome: receipt.outcome.clone(),
                replayed: true,
            });
        }
        if state
            .global
            .command_receipts
            .values()
            .any(|receipt| receipt.operation_id == operation_id)
        {
            return Err(MutationError::InvalidCommand(format!(
                "operation {operation} already belongs to another command"
            )));
        }
        if expected_revision.is_some_and(|revision| revision != state.revision) {
            return Err(MutationError::InvalidCommand(format!(
                "CAS_MISMATCH: expected {}, observed {}",
                expected_revision.unwrap(),
                state.revision
            )));
        }
        state::responsibility_preflight(state, changes).map_err(MutationError::InvalidCommand)?;
        let mut candidate = state.clone();
        for change in changes {
            candidate
                .apply_checked(change)
                .map_err(MutationError::InvalidState)?;
        }
        candidate
            .advance_version()
            .map_err(MutationError::InvalidState)?;
        candidate.global.command_receipts.insert(
            command.into(),
            global_state::CommandReceipt {
                command_id,
                operation_id,
                epoch: candidate.global.epoch,
                sequence: candidate.sequence,
                revision: candidate.revision,
                outcome: outcome.clone(),
            },
        );
        let receipt = self.publish_current_state(state, candidate)?;
        Ok(notification_contract::CommandOutcome {
            receipt,
            operation_id: operation.into(),
            outcome,
            replayed: false,
        })
    }

    fn publish_current_state(
        &self,
        state: &mut State,
        mut candidate: State,
    ) -> Result<notification_contract::CommitReceipt, notification_contract::MutationError> {
        candidate.mailbox_handoff_wakes.retain(|id, attempt| {
            candidate.msgs.get(id).is_some_and(|message| {
                matches!(message.state.as_str(), "pending" | "delivered")
                    && message.wake_attempt_count == *attempt
            })
        });
        candidate.global.validate().map_err(|error| {
            notification_contract::MutationError::InvalidState(error.to_string())
        })?;
        let confirmed = current_state::save(&self.state_path, &candidate)?;
        let receipt = notification_contract::CommitReceipt {
            sequence: candidate.sequence,
            revision: candidate.revision,
        };
        *state = candidate;
        self.mailbox_notify.notify_waiters();
        if !confirmed {
            return Err(notification_contract::MutationError::DurabilityUnknown(
                self.state_path.display().to_string(),
            ));
        }
        Ok(receipt)
    }

    pub(crate) fn report_state_error(&self, error: &notification_contract::MutationError) {
        append_log(&self.log_path(), &error.to_string());
    }
}
