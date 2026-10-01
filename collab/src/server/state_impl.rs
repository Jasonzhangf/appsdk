use super::*;
impl State {
    pub(crate) fn restore_unique_current_thread_routes_from_bindings(
        &mut self,
    ) -> Result<(), String> {
        let mut bindings = self
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .filter(|binding| binding.native_thread_id.is_some())
            .cloned()
            .collect::<Vec<_>>();
        bindings.sort_by(|left, right| {
            left.native_thread_id
                .as_ref()
                .map(|thread| thread.as_str())
                .cmp(
                    &right
                        .native_thread_id
                        .as_ref()
                        .map(|thread| thread.as_str()),
                )
        });
        let mut recovered = self.global.clone();
        for binding in bindings {
            let thread = binding.native_thread_id.clone().unwrap();
            let Some(session) = binding.session_id.clone() else {
                // A durable thread-only binding is a legacy record.  Keep it
                // resolvable through the read-only compatibility index instead
                // of aborting the whole host journal; the next explicit
                // registration or rebind upgrades it to the strict dual key.
                recovered
                    .set_legacy_thread_route(binding)
                    .map_err(|error| {
                        format!("journal replay rejected legacy thread route: {error}")
                    })?;
                continue;
            };
            let existing = match binding.tmux_endpoint.as_ref() {
                Some(endpoint) => recovered.lookup_tmux_route(endpoint),
                None => recovered.lookup_current_thread_route(&session, &thread),
            };
            if let Some(existing) = existing {
                if existing == &binding {
                    continue;
                }
                return Err(format!(
                    "journal replay rejected ambiguous current thread route {thread}"
                ));
            }
            recovered
                .set_current_thread_route(binding)
                .map_err(|error| {
                    format!("journal replay rejected current thread route: {error}")
                })?;
        }
        recovered.set_counters(self.sequence, self.revision);
        self.global = recovered;
        Ok(())
    }

    /// Project every durable thread-only binding into the read-only legacy
    /// compatibility index.
    ///
    /// This runs on every replay, independently of
    /// `restore_unique_current_thread_routes_from_bindings`.  That helper only
    /// backfills strict routes when a journal has no route event at all; a
    /// journal whose route events are themselves thread-only (the live host
    /// journal) would otherwise never index them, leaving every pre-dual-key
    /// route unresolvable after a restart.
    pub(crate) fn index_legacy_thread_routes_from_bindings(&mut self) -> Result<(), String> {
        let mut bindings = self
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .filter(|binding| binding.native_thread_id.is_some() && binding.session_id.is_none())
            .cloned()
            .collect::<Vec<_>>();
        bindings.sort_by(|left, right| {
            (
                left.native_thread_id.as_ref().map(|thread| thread.as_str()),
                left.binding_id.as_str(),
            )
                .cmp(&(
                    right
                        .native_thread_id
                        .as_ref()
                        .map(|thread| thread.as_str()),
                    right.binding_id.as_str(),
                ))
        });
        let mut recovered = self.global.clone();
        for binding in bindings {
            recovered
                .set_legacy_thread_route(binding)
                .map_err(|error| format!("journal replay rejected legacy thread route: {error}"))?;
        }
        recovered.set_counters(self.sequence, self.revision);
        self.global = recovered;
        Ok(())
    }

    /// Keep the typed projection on the daemon journal's version axis.  The
    /// resident reducer is the only owner of these counters; the nested
    /// global state mirrors them for typed CAS and receipts.
    pub(crate) fn sync_global_version(&mut self) {
        self.global.set_counters(self.sequence, self.revision);
    }

    pub(crate) fn advance_version(&mut self) -> Result<(), String> {
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| "sequence counter overflow".to_string())?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| "revision counter overflow".to_string())?;
        self.sequence = sequence;
        self.revision = revision;
        self.sync_global_version();
        Ok(())
    }

    pub(crate) fn set_checkpoint_version(
        &mut self,
        sequence: u64,
        revision: u64,
    ) -> Result<(), String> {
        if sequence < self.sequence || revision < self.revision {
            return Err(format!(
                "reducer checkpoint regresses version: current ({}, {}), observed ({sequence}, {revision})",
                self.sequence, self.revision
            ));
        }
        self.sequence = sequence;
        self.revision = revision;
        self.sync_global_version();
        Ok(())
    }

    fn consume_notification(&mut self, subscription_id: &str, consumed_ms: Option<i64>) {
        let Some(subscription) = self.notification_subscriptions.get_mut(subscription_id) else {
            return;
        };
        if subscription.status == "cancelled" {
            return;
        }
        subscription.fired_count = subscription.fired_count.saturating_add(1);
        if is_goal_deadline(subscription) {
            subscription.status = "consumed".into();
            subscription.status_reason = Some("goal-deadline-one-shot-delivered".into());
            if let Some(consumed_ms) = consumed_ms {
                subscription.updated_ms = consumed_ms;
            }
            return;
        }
        let total = if subscription.interval_ms.is_some() {
            subscription.repeat_count
        } else {
            subscription.trigger_times_ms.len().max(1) as u32
        };
        if subscription.fired_count >= total {
            subscription.status = "consumed".into();
        } else if let Some(interval) = subscription.interval_ms {
            let next_trigger = consumed_ms
                .filter(|_| subscription.event == "master-idle")
                .map(|consumed| consumed.saturating_add(interval))
                .or_else(|| {
                    subscription
                        .trigger_ms
                        .map(|trigger| trigger.saturating_add(interval))
                })
                .unwrap_or_else(|| {
                    subscription.created_ms.saturating_add(
                        interval.saturating_mul(subscription.fired_count.saturating_add(1) as i64),
                    )
                });
            subscription.trigger_ms = Some(next_trigger);
            subscription.status = "armed".into();
        } else {
            subscription.status = "armed".into();
        }
        if let Some(consumed_ms) = consumed_ms {
            subscription.updated_ms = consumed_ms;
        }
    }

    /// Apply one event for legacy callers.  The journal writer uses
    /// [`Self::apply_checked`] so a global reducer rejection is returned to
    /// the command boundary instead of being mistaken for success.
    pub fn apply(&mut self, ev: &Event) {
        if let Err(error) = self.apply_checked(ev) {
            self.journal_poison.get_or_insert(error);
        }
    }

    pub fn apply_checked(&mut self, ev: &Event) -> Result<(), String> {
        match ev {
            Event::MasterWakeSignal { signal, at_ms } => {
                crate::server::notification_state::accumulate_master_wake(
                    &mut self.master_wake,
                    signal,
                    *at_ms,
                );
            }
            Event::KeepaliveUpdated { worker_id, record } => {
                self.keepalives.insert(worker_id.clone(), record.clone());
            }
            Event::MasterWakeUpdated { accumulator } => {
                self.master_wake = accumulator.clone();
            }
            Event::SubagentUpdated { subagent } => {
                self.subagents.insert(subagent.id.clone(), subagent.clone());
                if subagent.status == "closed" {
                    let id = format!("subagent:{}", subagent.id);
                    self.master_wake.idle_workers.retain(|worker| worker != &id);
                    self.master_wake
                        .newly_idle_workers
                        .retain(|worker| worker != &id);
                }
            }
            Event::SubagentSnapshotCaptured {
                subagent_id,
                thread_id,
                captured_ms,
            } => {
                self.subagent_snapshots.insert(
                    subagent_id.clone(),
                    SubagentSnapshotReceipt {
                        subagent_id: subagent_id.clone(),
                        thread_id: thread_id.clone(),
                        captured_ms: *captured_ms,
                    },
                );
            }
            Event::WorkerSnapshotCaptured {
                worker_id,
                thread_id,
                captured_ms,
            } => {
                self.worker_snapshots.insert(
                    worker_id.clone(),
                    WorkerSnapshotReceipt {
                        worker_id: worker_id.clone(),
                        thread_id: thread_id.clone(),
                        captured_ms: *captured_ms,
                    },
                );
            }
            Event::Registered { worker } => {
                self.worker_closures.remove(&worker.id);
                self.worker_snapshots.remove(&worker.id);
                self.workers.insert(worker.id.clone(), worker.clone());
            }
            Event::LegacyWorkerRemoved { worker_id } => {
                self.workers.remove(worker_id);
                self.master_wake.idle_workers.retain(|id| id != worker_id);
                self.master_wake
                    .newly_idle_workers
                    .retain(|id| id != worker_id);
            }
            Event::WorkerClosed {
                worker_id,
                closed_by,
                reason,
                snapshot_captured_ms,
                at_ms,
            } => {
                self.worker_closures.insert(
                    worker_id.clone(),
                    WorkerCloseReceipt {
                        worker_id: worker_id.clone(),
                        closed_by: closed_by.clone(),
                        reason: reason.clone(),
                        snapshot_captured_ms: *snapshot_captured_ms,
                        at_ms: *at_ms,
                    },
                );
                self.workers.remove(worker_id);
                self.keepalives.remove(worker_id);
                self.master_wake.idle_workers.retain(|id| id != worker_id);
                self.master_wake
                    .newly_idle_workers
                    .retain(|id| id != worker_id);
            }
            Event::LegacyMasterTransferred { .. } => {}
            Event::Sent { msg } => {
                self.msgs.insert(msg.id.clone(), msg.clone());
            }
            Event::DeliveryMode {
                msg_id,
                mode,
                source_thread_id,
            } => {
                self.delivery_modes.insert(msg_id.clone(), mode.clone());
                if let Some(source_thread_id) = source_thread_id {
                    self.delivery_source_threads
                        .insert(msg_id.clone(), source_thread_id.clone());
                }
            }
            Event::WakeAttempted {
                ids,
                attempted_ms,
                retry,
            } => {
                for id in ids {
                    if let Some(message) = self.msgs.get_mut(id) {
                        message.wake_attempt_count = message.wake_attempt_count.saturating_add(1);
                        message.last_wake_attempt_ms = *attempted_ms;
                        message.retry_attempted |= *retry;
                    }
                }
            }
            Event::NotificationDeliveryFailed {
                message_id,
                operation,
                error,
                failed_ms,
                retryable,
            } => {
                let failed_master_wake = self.msgs.get(message_id).is_some_and(|message| {
                    message.from == "collab-server" && message.mtype == "notification"
                }) && self
                    .wake_bindings
                    .get(message_id)
                    .and_then(|subscription_id| {
                        self.notification_subscriptions.get(subscription_id)
                    })
                    .is_some_and(|subscription| {
                        subscription_affects_master_wake(self, subscription)
                    });
                if failed_master_wake {
                    crate::server::notification_state::mark_master_wake_delivery_failed(
                        &mut self.master_wake,
                    );
                }
                self.notification_delivery_failures.insert(
                    message_id.clone(),
                    NotificationDeliveryFailure {
                        message_id: message_id.clone(),
                        operation: operation.clone(),
                        error: error.clone(),
                        failed_ms: *failed_ms,
                        retryable: *retryable,
                    },
                );
            }
            Event::NotificationDeliveryAccepted {
                message_id,
                accepted_ms,
                evidence,
            } => {
                self.notification_delivery_accepted
                    .insert(message_id.clone(), *accepted_ms);
                if let Some(evidence) = evidence {
                    self.notification_delivery_evidence
                        .insert(message_id.clone(), evidence.clone());
                }
            }
            Event::NotificationSubscribed { subscription } => {
                self.notification_subscriptions
                    .insert(subscription.id.clone(), subscription.clone());
            }
            Event::NotificationStatus {
                subscription_id,
                status,
                updated_ms,
            } => {
                if let Some(subscription) = self.notification_subscriptions.get_mut(subscription_id)
                {
                    subscription.status = status.clone();
                    subscription.status_reason = None;
                    subscription.updated_ms = *updated_ms;
                }
            }
            Event::NotificationRebound {
                subscription_id,
                target,
                updated_ms,
            } => {
                if let Some(subscription) = self.notification_subscriptions.get_mut(subscription_id)
                {
                    subscription.target = target.clone();
                    subscription.updated_ms = *updated_ms;
                }
            }
            Event::NotificationSuppressed {
                subscription_id,
                status,
                reason,
                updated_ms,
            } => {
                if let Some(subscription) = self.notification_subscriptions.get_mut(subscription_id)
                {
                    subscription.status = status.clone();
                    subscription.status_reason = Some(reason.clone());
                    subscription.updated_ms = *updated_ms;
                }
            }
            Event::NotificationSkipped {
                subscription_id,
                reason,
                due_ms,
                skipped_ms,
            } => {
                let affects_master_wake = self
                    .notification_subscriptions
                    .get(subscription_id)
                    .is_some_and(|subscription| {
                        subscription_affects_master_wake(self, subscription)
                    });
                let Some(subscription) = self.notification_subscriptions.get_mut(subscription_id)
                else {
                    return Ok(());
                };
                if affects_master_wake {
                    crate::server::notification_state::mark_master_wake_skipped_busy(&mut self.master_wake);
                }
                if is_goal_deadline(subscription) {
                    let revision = u64::try_from(*due_ms).unwrap_or(0);
                    crate::server::notification_state::accumulate_master_wake(
                        &mut self.master_wake,
                        &MasterWakeSignal::GoalDue { revision },
                        *skipped_ms,
                    );
                    crate::server::notification_state::mark_master_wake_skipped_busy(&mut self.master_wake);
                    subscription.status_reason = Some(reason.clone());
                    subscription.updated_ms = *skipped_ms;
                    return Ok(());
                }
                subscription.fired_count = subscription.fired_count.saturating_add(1);
                subscription.status_reason = Some(reason.clone());
                subscription.updated_ms = *skipped_ms;
                let total = if subscription.interval_ms.is_some() {
                    subscription.repeat_count
                } else {
                    subscription.trigger_times_ms.len().max(1) as u32
                };
                if subscription.fired_count >= total {
                    subscription.status = "consumed".into();
                } else if let Some(interval) = subscription.interval_ms {
                    subscription.trigger_ms = Some(
                        subscription
                            .trigger_ms
                            .map(|trigger| trigger.saturating_add(interval))
                            .unwrap_or_else(|| {
                                subscription
                                    .created_ms
                                    .saturating_add(interval.saturating_mul(
                                        subscription.fired_count.saturating_add(1) as i64,
                                    ))
                            }),
                    );
                    subscription.status = "armed".into();
                } else {
                    subscription.status = "armed".into();
                }
            }
            Event::NotificationConsumed {
                subscription_id,
                message_id: _,
                consumed_ms,
            } => {
                self.consume_notification(subscription_id, Some(*consumed_ms));
            }
            Event::WakeBound {
                message_id,
                subscription_id,
            } => {
                self.wake_bindings
                    .insert(message_id.clone(), subscription_id.clone());
            }
            Event::ReceiveCommitted { receipt, ids } => {
                self.receive_receipts
                    .insert(receipt.receive_id.clone(), receipt.clone());
                self.apply(&Event::Delivered { ids: ids.clone() });
                self.apply(&Event::Acked { ids: ids.clone() });
            }
            Event::Delivered { ids } => {
                for id in ids {
                    if let Some(m) = self.msgs.get_mut(id) {
                        if m.state == "pending" {
                            m.state = "delivered".into();
                        }
                    }
                }
                if ids.iter().any(|id| {
                    self.wake_bindings.get(id).is_some_and(|subscription_id| {
                        self.notification_subscriptions
                            .get(subscription_id)
                            .is_some_and(|subscription| {
                                self.msgs.get(id).is_some_and(|message| {
                                    message.from == "collab-server"
                                        && message.to == subscription.worker_id
                                        && has_current_master_grant(
                                            self,
                                            &subscription.worker_id,
                                            &subscription.target,
                                        )
                                })
                            })
                    })
                }) {
                    crate::server::notification_state::mark_master_wake_delivered(&mut self.master_wake);
                }
            }
            Event::Acked { ids } => {
                for id in ids {
                    if let Some(m) = self.msgs.get_mut(id) {
                        m.state = "read".into();
                    }
                    let Some(subscription_id) = self.wake_bindings.get(id).cloned() else {
                        continue;
                    };
                    let Some(subscription) = self.notification_subscriptions.get(&subscription_id)
                    else {
                        continue;
                    };
                    let consumes_on_read = subscription.event != "direct-message"
                        || subscription.trigger_ms.is_some()
                        || !subscription.trigger_times_ms.is_empty()
                        || subscription.interval_ms.is_some();
                    if !consumes_on_read {
                        continue;
                    }
                    let fired_count = subscription.fired_count;
                    let read_count = self
                        .wake_bindings
                        .iter()
                        .filter(|(_, bound)| *bound == &subscription_id)
                        .filter(|(message_id, _)| {
                            self.msgs
                                .get(*message_id)
                                .is_some_and(|message| message.state == "read")
                        })
                        .count() as u32;
                    // recv records Delivered + Acked without a separate
                    // NotificationConsumed event. Compare durable read
                    // occurrences with the cursor so timer delivery, which
                    // already records NotificationConsumed, remains idempotent.
                    if read_count > fired_count {
                        let consumed_ms = if subscription.event == "master-idle" {
                            self.msgs.get(id).and_then(|message| {
                                [message.last_wake_attempt_ms, message.created_ms]
                                    .into_iter()
                                    .find(|timestamp| *timestamp > 0)
                            })
                        } else {
                            None
                        };
                        self.consume_notification(&subscription_id, consumed_ms);
                    }
                }
            }
            Event::Superseded { ids } => {
                for id in ids {
                    if let Some(m) = self.msgs.get_mut(id) {
                        m.state = "superseded".into();
                    }
                }
            }
            Event::LegacyNudged { msg_id } => {
                if let Some(m) = self.msgs.get_mut(msg_id) {
                    m.wake_attempt_count = m.wake_attempt_count.saturating_add(1);
                    m.last_wake_attempt_ms = 0;
                }
            }
            Event::TaskCreated { task } | Event::TaskUpdated { task } => {
                self.tasks.insert(task.id.clone(), task.clone());
            }
            Event::SchedulerAdmission { admission } => {
                self.scheduler_admissions
                    .insert(admission.request_id.clone(), admission.clone());
            }
            Event::SchedulerAdmissionStatus {
                request_id,
                status,
                error,
                updated_ms,
            } => {
                if let Some(admission) = self.scheduler_admissions.get_mut(request_id) {
                    admission.status = status.clone();
                    admission.error = error.clone();
                    admission.updated_ms = *updated_ms;
                }
            }
            Event::TaskLifecycleUpdated { task_id, record } => {
                self.task_lifecycle.insert(task_id.clone(), record.clone());
            }
            Event::MergeRequested { request } => {
                self.pending_merges
                    .insert(request.task_id.clone(), request.clone());
            }
            Event::MergeResolved { task_id, .. } => {
                self.pending_merges.remove(task_id);
            }
            Event::CleanupVerified { receipt } => {
                self.cleanup_receipts
                    .insert(receipt.task_id.clone(), receipt.clone());
            }
            Event::MigrationUpdated { migration } => {
                self.migration = Some(migration.clone());
            }
            Event::ReducerCheckpoint { .. } => {}
            Event::CommandStarted { .. } => {}
            Event::CommandRecorded {
                command_id,
                receipt,
            } => {
                self.project_command_receipt(command_id, receipt)?;
                self.legacy_command_ids.insert(command_id.clone());
                self.command_receipts
                    .insert(command_id.clone(), receipt.clone());
            }
            Event::CommandCompleted {
                command_id,
                operation_id: _,
                receipt,
            } => {
                self.project_command_receipt(command_id, receipt)?;
                self.command_receipts
                    .insert(command_id.clone(), receipt.clone());
            }
            Event::MasterAssigned {
                worker_id,
                assigned_by,
                approval,
                assigned_ms,
            } => {
                self.master_worker_id = Some(worker_id.clone());
                self.master_assigned_by = Some(assigned_by.clone());
                self.master_approval = approval.clone();
                self.master_assigned_ms = Some(*assigned_ms);
            }
            Event::WorktreeBound { binding } => {
                self.worktree_bindings
                    .insert(binding.binding_id.clone(), binding.clone());
            }
            Event::GlobalProjectRegistered { registration } => {
                self.apply_global_event(&GlobalEvent::ProjectRegistered {
                    registration: registration.clone(),
                })?;
            }
            Event::GlobalRuntimeBound { binding } => {
                self.apply_global_event(&GlobalEvent::RuntimeBound {
                    binding: binding.clone(),
                })?;
            }
            Event::GlobalMasterGranted { grant } => {
                self.apply_global_event(&GlobalEvent::MasterGranted {
                    grant: grant.clone(),
                })?;
            }
            Event::GlobalMasterRevoked {
                project_scope,
                binding_id,
            } => {
                let revoked_agent = self
                    .global
                    .lookup_project(project_scope)
                    .and_then(|project| project.runtime_bindings.get(binding_id.as_str()))
                    .map(|binding| binding.agent_id.as_str().to_owned());
                if revoked_agent.as_deref() == self.master_worker_id.as_deref() {
                    self.master_worker_id = None;
                    self.master_assigned_by = None;
                    self.master_approval = None;
                    self.master_assigned_ms = None;
                }
                self.apply_global_event(&GlobalEvent::MasterRevoked {
                    project_scope: project_scope.clone(),
                    binding_id: binding_id.clone(),
                })?;
            }
            Event::GlobalRuntimeBindingRollback {
                failed,
                previous,
                previous_grant,
                previous_worker,
                previous_subscriptions,
            } => {
                let worker_id = failed.agent_id.as_str().to_owned();
                let mut next = self.global.clone();
                next.rollback_runtime_binding(
                    failed.clone(),
                    previous.clone(),
                    previous_grant.clone(),
                )
                .map_err(|error| format!("global reducer rejected event: {error}"))?;
                let failed_is_local_route = failed.session_id.as_ref().zip(
                    failed.native_thread_id.as_ref(),
                ).is_some_and(|(session, thread)| {
                    next.lookup_current_thread_route(session, thread) == Some(failed)
                });
                if failed_is_local_route {
                    next.retire_current_thread_route(failed.clone())
                        .map_err(|error| format!("global reducer rejected route rollback: {error}"))?;
                    if let Some(previous) = previous.as_ref() {
                        next.set_current_thread_route(previous.clone()).map_err(|error| {
                            format!("global reducer rejected route restoration: {error}")
                        })?;
                    }
                }
                next.set_counters(self.sequence, self.revision);
                self.global = next;
                if let Some(previous_worker) = previous_worker {
                    self.workers
                        .insert(worker_id.clone(), previous_worker.clone());
                } else {
                    self.workers.remove(&worker_id);
                }
                self.notification_subscriptions
                    .retain(|_, subscription| subscription.worker_id != worker_id);
                for subscription in previous_subscriptions {
                    self.notification_subscriptions
                        .insert(subscription.id.clone(), subscription.clone());
                }
            }
            Event::GlobalCurrentThreadRouteSet { binding } => {
                let mut next = self.global.clone();
                if binding.session_id.is_none() {
                    // A durable thread-only route predates the strict dual key.
                    // Replay must keep it resolvable through the read-only
                    // compatibility index instead of aborting the host
                    // journal; the next explicit rebind upgrades it.
                    next.set_legacy_thread_route(binding.clone())
                        .map_err(|error| {
                            format!("global reducer rejected legacy thread route: {error}")
                        })?;
                } else {
                    next.set_current_thread_route(binding.clone())
                        .map_err(|error| format!("global reducer rejected event: {error}"))?;
                }
                next.set_counters(self.sequence, self.revision);
                self.global = next;
            }
            Event::GlobalCurrentThreadRouteRetired { binding } => {
                let mut next = self.global.clone();
                next.retire_current_thread_route(binding.clone())
                    .map_err(|error| format!("global reducer rejected event: {error}"))?;
                next.set_counters(self.sequence, self.revision);
                self.global = next;
            }
            Event::GlobalCurrentThreadRouteTombstoneSet { tombstone } => {
                let mut next = self.global.clone();
                next.record_current_thread_route_tombstone(tombstone.clone())
                    .map_err(|error| format!("global reducer rejected event: {error}"))?;
                next.set_counters(self.sequence, self.revision);
                self.global = next;
            }
            Event::GlobalMigrationCommitEvidence { evidence } => {
                self.apply_global_event(&GlobalEvent::MigrationCommitEvidence {
                    evidence: evidence.clone(),
                })?;
            }
            Event::GlobalRuntimeBindingLedgerClassified { record } => {
                self.apply_global_event(&GlobalEvent::RuntimeBindingLedgerClassified { record: record.clone() })?;
            }
            Event::GlobalLedgerScanReceiptRecorded { receipt } => {
                self.apply_global_event(&GlobalEvent::LedgerScanReceiptRecorded { receipt: receipt.clone() })?;
            }
        }
        Ok(())
    }

    fn project_command_receipt(
        &mut self,
        command_id: &str,
        receipt: &CommandReceipt,
    ) -> Result<(), String> {
        let command_id = crate::identity::CommandId::new(command_id.to_owned())
            .map_err(|error| format!("global command receipt has invalid command id: {error}"))?;
        let operation_id = crate::identity::OperationId::new(receipt.operation_id.clone())
            .map_err(|error| format!("global command receipt has invalid operation id: {error}"))?;
        self.global
            .record_command_projection(crate::server::global_state::CommandReceipt {
                command_id,
                operation_id,
                epoch: self.global.epoch,
                sequence: receipt.sequence,
                revision: receipt.revision,
                outcome: receipt.outcome.clone(),
            })
            .map_err(|error| format!("global command receipt rejected: {error}"))
    }

    pub fn apply_global_event(&mut self, event: &GlobalEvent) -> Result<(), String> {
        // GlobalEvent::apply already mutates through the atomic clone-then-commit
        // primitive, so cloning the whole global state a second time here only
        // added one more O(state) copy per event and made journal replay
        // quadratic for journals with many repeated global events.
        let sequence = self.sequence;
        let revision = self.revision;
        event
            .clone()
            .apply(&mut self.global)
            .map_err(|error| format!("global reducer rejected event: {error}"))?;
        self.global.set_counters(sequence, revision);
        Ok(())
    }

    pub fn drop_message(&mut self, id: &str) {
        self.msgs.remove(id);
        self.notification_delivery_failures.remove(id);
        self.notification_delivery_accepted.remove(id);
        self.notification_delivery_evidence.remove(id);
        self.delivery_modes.remove(id);
        self.delivery_source_threads.remove(id);
        self.wake_bindings.remove(id);
    }

    pub fn snapshot_events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        if master_wake_snapshot_required(&self.master_wake) {
            events.push(Event::MasterWakeUpdated {
                accumulator: self.master_wake.clone(),
            });
        }
        let mut closures: Vec<_> = self.worker_closures.values().cloned().collect();
        closures.sort_by(|a, b| a.worker_id.cmp(&b.worker_id));
        events.extend(closures.into_iter().map(|receipt| Event::WorkerClosed {
            worker_id: receipt.worker_id,
            closed_by: receipt.closed_by,
            reason: receipt.reason,
            snapshot_captured_ms: receipt.snapshot_captured_ms,
            at_ms: receipt.at_ms,
        }));
        let mut workers: Vec<_> = self.workers.values().cloned().collect();
        workers.sort_by(|a, b| a.id.cmp(&b.id));
        events.extend(
            workers
                .into_iter()
                .map(|worker| Event::Registered { worker }),
        );
        let mut keepalives: Vec<_> = self.keepalives.iter().collect();
        keepalives.sort_by(|a, b| a.0.cmp(b.0));
        events.extend(
            keepalives
                .into_iter()
                .map(|(worker_id, record)| Event::KeepaliveUpdated {
                    worker_id: worker_id.clone(),
                    record: record.clone(),
                }),
        );
        let mut subagents: Vec<_> = self.subagents.values().cloned().collect();
        subagents.sort_by(|a, b| a.id.cmp(&b.id));
        events.extend(
            subagents
                .into_iter()
                .map(|subagent| Event::SubagentUpdated { subagent }),
        );
        let mut snapshots: Vec<_> = self.subagent_snapshots.values().cloned().collect();
        snapshots.sort_by(|a, b| a.subagent_id.cmp(&b.subagent_id));
        events.extend(
            snapshots
                .into_iter()
                .map(|receipt| Event::SubagentSnapshotCaptured {
                    subagent_id: receipt.subagent_id,
                    thread_id: receipt.thread_id,
                    captured_ms: receipt.captured_ms,
                }),
        );
        let mut worker_snapshots: Vec<_> = self.worker_snapshots.values().cloned().collect();
        worker_snapshots.sort_by(|a, b| a.worker_id.cmp(&b.worker_id));
        events.extend(
            worker_snapshots
                .into_iter()
                .map(|receipt| Event::WorkerSnapshotCaptured {
                    worker_id: receipt.worker_id,
                    thread_id: receipt.thread_id,
                    captured_ms: receipt.captured_ms,
                }),
        );
        let mut tasks: Vec<_> = self.tasks.values().cloned().collect();
        tasks.sort_by(|a, b| a.id.cmp(&b.id));
        events.extend(tasks.into_iter().map(|task| Event::TaskCreated { task }));
        let mut scheduler_admissions: Vec<_> =
            self.scheduler_admissions.values().cloned().collect();
        scheduler_admissions.sort_by(|a, b| a.request_id.cmp(&b.request_id));
        events.extend(
            scheduler_admissions
                .into_iter()
                .map(|admission| Event::SchedulerAdmission { admission }),
        );
        let mut lifecycle: Vec<_> = self.task_lifecycle.iter().collect();
        lifecycle.sort_by(|a, b| a.0.cmp(b.0));
        events.extend(
            lifecycle
                .into_iter()
                .map(|(task_id, record)| Event::TaskLifecycleUpdated {
                    task_id: task_id.clone(),
                    record: record.clone(),
                }),
        );
        let mut merges: Vec<_> = self.pending_merges.values().cloned().collect();
        merges.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        events.extend(
            merges
                .into_iter()
                .map(|request| Event::MergeRequested { request }),
        );
        let mut receipts: Vec<_> = self.cleanup_receipts.values().cloned().collect();
        receipts.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        events.extend(
            receipts
                .into_iter()
                .map(|receipt| Event::CleanupVerified { receipt }),
        );
        let mut subscriptions: Vec<_> = self.notification_subscriptions.values().cloned().collect();
        subscriptions.sort_by(|a, b| a.id.cmp(&b.id));
        events.extend(
            subscriptions
                .into_iter()
                .map(|subscription| Event::NotificationSubscribed { subscription }),
        );
        let mut delivery_failures: Vec<_> = self
            .notification_delivery_failures
            .values()
            .cloned()
            .collect();
        delivery_failures.sort_by(|a, b| {
            (a.failed_ms, a.message_id.as_str()).cmp(&(b.failed_ms, b.message_id.as_str()))
        });
        events.extend(delivery_failures.into_iter().map(|failure| {
            Event::NotificationDeliveryFailed {
                message_id: failure.message_id,
                operation: failure.operation,
                error: failure.error,
                failed_ms: failure.failed_ms,
                retryable: failure.retryable,
            }
        }));
        let mut accepted: Vec<_> = self
            .notification_delivery_accepted
            .iter()
            .map(|(message_id, accepted_ms)| (message_id.clone(), *accepted_ms))
            .collect();
        accepted.sort_by(|a, b| (a.1, a.0.as_str()).cmp(&(b.1, b.0.as_str())));
        events.extend(accepted.into_iter().map(|(message_id, accepted_ms)| {
            let evidence = self
                .notification_delivery_evidence
                .get(&message_id)
                .cloned();
            Event::NotificationDeliveryAccepted {
                message_id,
                accepted_ms,
                evidence,
            }
        }));
        let mut messages: Vec<_> = self.msgs.values().cloned().collect();
        messages.sort_by(|a, b| (a.created_ms, a.id.clone()).cmp(&(b.created_ms, b.id.clone())));
        for msg in messages {
            let id = msg.id.clone();
            events.push(Event::Sent { msg });
            if let Some(mode) = self.delivery_modes.get(&id) {
                events.push(Event::DeliveryMode {
                    msg_id: id.clone(),
                    mode: mode.clone(),
                    source_thread_id: self.delivery_source_threads.get(&id).cloned(),
                });
            }
            if let Some(subscription_id) = self.wake_bindings.get(&id) {
                events.push(Event::WakeBound {
                    message_id: id,
                    subscription_id: subscription_id.clone(),
                });
            }
        }
        if let Some(migration) = self.migration.clone() {
            events.push(Event::MigrationUpdated { migration });
        }
        let mut command_receipts: Vec<_> = self.command_receipts.iter().collect();
        command_receipts.sort_by(|a, b| a.0.cmp(b.0));
        for (command_id, receipt) in command_receipts {
            if self.legacy_command_ids.contains(command_id) {
                events.push(Event::CommandRecorded {
                    command_id: command_id.clone(),
                    receipt: receipt.clone(),
                });
            } else {
                events.push(Event::CommandStarted {
                    command_id: command_id.clone(),
                    operation_id: receipt.operation_id.clone(),
                });
                events.push(Event::CommandCompleted {
                    command_id: command_id.clone(),
                    operation_id: receipt.operation_id.clone(),
                    receipt: receipt.clone(),
                });
            }
        }
        let mut bindings: Vec<_> = self.worktree_bindings.values().cloned().collect();
        bindings.sort_by(|a, b| a.binding_id.cmp(&b.binding_id));
        events.extend(
            bindings
                .into_iter()
                .map(|binding| Event::WorktreeBound { binding }),
        );
        for (_, project) in &self.global.projects {
            for registration in project.registrations.values() {
                events.push(Event::GlobalProjectRegistered {
                    registration: registration.clone(),
                });
            }
            for binding in project.runtime_bindings.values() {
                events.push(Event::GlobalRuntimeBound {
                    binding: binding.clone(),
                });
            }
            for grant in project.master_grants.values() {
                events.push(Event::GlobalMasterGranted {
                    grant: grant.clone(),
                });
            }
        }
        events.extend(
            self.global
                .current_thread_routes
                .values()
                .cloned()
                .map(|binding| Event::GlobalCurrentThreadRouteSet { binding }),
        );
        // Legacy thread-only routes must survive compaction too.  Replaying a
        // snapshot that already contains a strict route skips the
        // bindings-based restoration, so omitting these would silently drop
        // every pre-dual-key route on the next restart.
        let mut legacy_routes: Vec<_> =
            self.global.legacy_thread_routes.values().cloned().collect();
        legacy_routes.sort_by(|left, right| {
            (
                left.app_scope_id.as_str(),
                left.native_thread_id.as_ref().map(|thread| thread.as_str()),
                left.binding_id.as_str(),
            )
                .cmp(&(
                    right.app_scope_id.as_str(),
                    right
                        .native_thread_id
                        .as_ref()
                        .map(|thread| thread.as_str()),
                    right.binding_id.as_str(),
                ))
        });
        events.extend(
            legacy_routes
                .into_iter()
                .map(|binding| Event::GlobalCurrentThreadRouteSet { binding }),
        );
        events.extend(
            self.global
                .current_thread_route_tombstones
                .values()
                .cloned()
                .map(|tombstone| Event::GlobalCurrentThreadRouteTombstoneSet { tombstone }),
        );
        let mut migration_commit_evidence: Vec<_> = self
            .global
            .migration_commit_evidence
            .values()
            .cloned()
            .collect();
        migration_commit_evidence
            .sort_by(|a, b| a.operation_id.as_str().cmp(b.operation_id.as_str()));
        events.extend(
            migration_commit_evidence
                .into_iter()
                .map(|evidence| Event::GlobalMigrationCommitEvidence { evidence }),
        );
        events.push(Event::ReducerCheckpoint {
            sequence: self.sequence,
            revision: self.revision,
        });
        events
    }

    pub fn admission_frozen(&self) -> bool {
        self.journal_poison.is_some()
            || self
                .migration
                .as_ref()
                .is_some_and(|migration| migration.admission_frozen)
    }

    /// Unread (not yet acked) inbox of a worker, oldest first.
    pub fn inbox_of(&self, worker_id: &str) -> Vec<&Message> {
        let mut v: Vec<&Message> = self
            .msgs
            .values()
            .filter(|m| {
                m.to == worker_id
                    && m.state != "read"
                    && m.state != "superseded"
                    && self.scheduler_message_deliverable(&m.id)
            })
            .collect();
        v.sort_by_key(|m| m.created_ms);
        v
    }

    pub fn scheduler_message_deliverable(&self, message_id: &str) -> bool {
        !self
            .scheduler_admissions
            .values()
            .any(|admission| admission.message_id == message_id && admission.status == "failed")
    }

    /// True when some other message is a reply to `msg`.
    pub fn answered(&self, msg_id: &str) -> bool {
        self.msgs
            .values()
            .any(|m| m.in_reply_to.as_deref() == Some(msg_id))
    }

    /// One live request per direction during the cooldown window.
    pub fn recent_live_request(
        &self,
        from: &str,
        to: &str,
        now_ms: i64,
    ) -> Option<(&String, &Message)> {
        self.msgs.iter().find(|(_, m)| {
            m.from == from
                && m.to == to
                && m.mtype == "request"
                && m.state != "read"
                && !self.answered(&m.id)
                && now_ms - m.created_ms < REQUEST_COOLDOWN_MS
        })
    }

    /// Earlier replies remain journaled, but only the newest one is active.
    pub fn superseded_replies(&self, request_id: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .msgs
            .values()
            .filter(|m| {
                m.mtype == "reply"
                    && m.in_reply_to.as_deref() == Some(request_id)
                    && m.state != "superseded"
            })
            .map(|m| m.id.clone())
            .collect();
        ids.sort_by(|a, b| {
            let rank = |id: &str| {
                self.msgs
                    .get(id)
                    .map(|m| (m.created_ms, m.id.clone()))
                    .unwrap_or_default()
            };
            rank(a).cmp(&rank(b))
        });
        ids
    }

    pub fn matching_subscription(
        &self,
        worker_id: &str,
        event: &str,
        subject: Option<&str>,
        now: i64,
    ) -> Option<&NotificationSubscription> {
        self.notification_subscriptions
            .values()
            .filter(|subscription| subscription.matches(worker_id, event, subject, now))
            .min_by_key(|subscription| (subscription.created_ms, subscription.id.as_str()))
    }
}

fn master_wake_snapshot_required(
    accumulator: &crate::server::notification_state::MasterWakeAccumulator,
) -> bool {
    accumulator.generation > 0
        || (!accumulator.delivery_state.is_empty() && accumulator.delivery_state != "clean")
}
