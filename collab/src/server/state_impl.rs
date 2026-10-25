use super::*;
use crate::server::global_state::CurrentThreadRouteReplayStatus;

pub(crate) fn board_patrol_owner_key(state: &State, worker_id: &str) -> String {
    let scoped = state
        .workers
        .get(worker_id)
        .and_then(|worker| state.name_occupancies.get(&worker.cwd))
        .and_then(|names| names.values().find(|holder| holder.worker_id == worker_id));
    let unique = scoped.or_else(|| {
        let mut holders = state
            .name_occupancies
            .values()
            .flat_map(|names| names.values())
            .filter(|holder| holder.worker_id == worker_id);
        let first = holders.next();
        if holders.next().is_none() {
            first
        } else {
            None
        }
    });
    unique
        .map(|holder| holder.name.clone())
        .or_else(|| state.worker_names.get(worker_id).cloned())
        .unwrap_or_else(|| worker_id.to_owned())
}

pub(crate) fn board_patrol_owned_unclosed_task_ids(
    state: &State,
    worker_id: &str,
    owner_key: &str,
) -> Vec<String> {
    let mut task_ids: Vec<String> = state
        .tasks
        .values()
        .filter(|task| {
            crate::board_patrol_policy::is_unclosed_board_status(&task.status)
                && state
                    .task_assignments
                    .get(&task.id)
                    .map_or(task.owner == worker_id, |assignment| {
                        assignment.name == owner_key
                    })
        })
        .map(|task| task.id.clone())
        .collect();
    task_ids.sort();
    task_ids
}

impl State {
    /// Clear a persisted goal-due wake that no longer has an armed goal
    /// deadline owned by the master. Old daemons could snapshot `goal_due`
    /// after the one-shot wake was already consumed; replay restores that
    /// stale flag unless it is reconciled against live subscriptions.
    pub(crate) fn reconcile_stale_master_goal_due(&mut self, now: i64) -> bool {
        if !self.master_wake.goal_due {
            return false;
        }
        let has_armed_master_goal_deadline = self
            .notification_subscriptions
            .values()
            .any(|subscription| {
                is_goal_deadline(subscription)
                    && subscription.status == "armed"
                    && subscription.expires_ms > now
                    && subscription_affects_master_wake(self, subscription)
            });
        if has_armed_master_goal_deadline {
            return false;
        }
        crate::server::notification_state::clear_master_goal_due(&mut self.master_wake);
        true
    }

    /// Backfill strict routes that replay did not establish.
    ///
    /// Route events are authoritative for existing ownership. Durable
    /// bindings only fill an address with no live claimant or tombstone, and a
    /// pane with several historical bindings is not uniquely restorable.
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
        let mut pane_binding_counts = std::collections::BTreeMap::new();
        for binding in &bindings {
            if binding.tmux_endpoint.is_some() {
                let key =
                    crate::server::global_state::installed_route_address_key(binding)
                        .expect("validated pane binding has a route address");
                *pane_binding_counts.entry(key).or_insert(0usize) += 1;
            }
        }
        let mut recovered = self.global.clone();
        for binding in bindings {
            let thread = binding.native_thread_id.clone().unwrap();
            if binding.session_id.is_none() {
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
            }
            match recovered.current_thread_route_replay_status(&binding) {
                CurrentThreadRouteReplayStatus::Installed
                | CurrentThreadRouteReplayStatus::Tombstoned
                | CurrentThreadRouteReplayStatus::Contested => continue,
                CurrentThreadRouteReplayStatus::Ambiguous(_)
                    if binding.tmux_endpoint.is_some() =>
                {
                    // The replay projection already owns this pane. Restoring
                    // historical candidates in thread order must not evict it.
                    continue;
                }
                CurrentThreadRouteReplayStatus::Ambiguous(existing) => {
                    return Err(format!(
                        "journal replay rejected ambiguous current thread route {thread}: {} is already live at this address",
                        existing.binding_id
                    ));
                }
                CurrentThreadRouteReplayStatus::Missing => {}
            }
            if binding.tmux_endpoint.is_some() {
                let key =
                    crate::server::global_state::installed_route_address_key(&binding)
                        .expect("validated pane binding has a route address");
                if pane_binding_counts.get(&key).copied() != Some(1) {
                    continue;
                }
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
    /// `restore_unique_current_thread_routes_from_bindings`.  That helper
    /// backfills strict routes from durable bindings; this index covers the
    /// thread-only binding shape, including journals whose route events are
    /// themselves thread-only (the live host journal).
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
        let clears_master_goal_due = self
            .notification_subscriptions
            .get(subscription_id)
            .is_some_and(|subscription| {
                subscription.status != "cancelled"
                    && is_goal_deadline(subscription)
                    && subscription_affects_master_wake(self, subscription)
            });
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
            if clears_master_goal_due {
                crate::server::notification_state::clear_master_goal_due(&mut self.master_wake);
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

    /// Move unsettled obligations that reference `name` in `project_scope` to
    /// the current carrier `worker_id`. The stable name keeps the obligation
    /// answerable across a session change; the old carrier loses it.
    fn rebind_reply_obligations_for_name(
        &mut self,
        project_scope: &str,
        name: &str,
        worker_id: &str,
    ) {
        for obligation in self.reply_obligations.values_mut() {
            if obligation.project != project_scope {
                continue;
            }
            // Reset the idle reminder only on a real carrier change; an
            // unchanged name resolution must not recreate a reminder window.
            if obligation.from_name.as_deref() == Some(name)
                && obligation.from_project.as_deref().is_none_or(|source|source==project_scope)
                && obligation.from_worker_id != worker_id
            {
                obligation.from_worker_id = worker_id.to_owned();
            }
            if obligation.recipient_name.as_deref() == Some(name)
                && obligation.recipient_worker_id != worker_id
            {
                obligation.recipient_worker_id = worker_id.to_owned();
                obligation.reminded_idle_episode = false;
            }
        }
    }

    /// Unsettled obligations owned by the current carrier `worker`. `worker`
    /// is a worker id; the rebinding above keeps the
    /// recipient id current for both, so a name that moved to a new session
    /// still answers and the old carrier no longer does.
    pub fn pending_reply_obligations(&self, worker: &str) -> Vec<ReplyObligation> {
        let mut obligations: Vec<ReplyObligation> = self
            .reply_obligations
            .values()
            .filter(|obligation| obligation.recipient_worker_id == worker)
            .cloned()
            .collect();
        obligations.sort_by(|a, b| {
            (a.created_ms, a.message_id.as_str()).cmp(&(b.created_ms, b.message_id.as_str()))
        });
        obligations
    }

    /// Read-only view of the pending replies owed by `worker`. `from` is the
    /// current carrier of the original sender, which is exactly the worker a
    /// linked reply must target.
    pub fn reply_obligation_view(&self, worker: &str) -> Vec<serde_json::Value> {
        self.pending_reply_obligations(worker)
            .into_iter()
            .map(|obligation| {
                serde_json::json!({
                    "message_id": obligation.message_id,
                    "from": obligation.from_worker_id,
                    "from_name": obligation.from_name,
                    "from_project": obligation.from_project,
                    "from_ceo": obligation.from_ceo,
                    "project": obligation.project,
                    "policy": obligation.policy.as_str(),
                    "created_ms": obligation.created_ms,
                    "reminded_idle_episode": obligation.reminded_idle_episode,
                })
            })
            .collect()
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
            Event::MailboxNameAddressed {
                message_id,
                address,
            } => {
                self.mailbox_name_addresses
                    .insert(message_id.clone(), address.clone());
            }
            Event::MailboxReassigned { handoff } => {
                // Older snapshots retain handoffs even after the name is released.
                self.retain_name_occupancy_revision(
                    &handoff.project_scope,
                    &handoff.name,
                    handoff.occupancy_revision,
                );
                self.apply_mailbox_handoff(handoff);
                self.rebind_reply_obligations_for_name(
                    &handoff.project_scope,
                    &handoff.name,
                    &handoff.recipient,
                );
            }
            Event::ProjectResetStarted {
                project_scope,
                clear_local,
                worker_ids,
            } => {
                crate::server::project_reset::clear_project(
                    self,
                    project_scope,
                    *clear_local,
                    worker_ids,
                );
                self.pending_project_resets
                    .insert(project_scope.clone(), *clear_local);
            }
            Event::ProjectResetCompleted { project_scope } => {
                self.pending_project_resets.remove(project_scope);
            }
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
                for task in self
                    .tasks
                    .values()
                    .filter(|task| task.created_by == subagent.peer)
                {
                    if let Some(details) = self.board_details.get_mut(&task.id) {
                        details.public_visibility = false;
                    }
                }
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
            Event::WorkerNamed { worker_id, name } => match name {
                Some(name) => {
                    self.worker_names.insert(worker_id.clone(), name.clone());
                }
                None => {
                    self.worker_names.remove(worker_id);
                }
            },
            Event::LegacyWorkerRemoved { worker_id } => {
                self.workers.remove(worker_id);
                self.worker_names.remove(worker_id);
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
                self.worker_names.remove(worker_id);
                self.keepalives.remove(worker_id);
                self.master_wake.idle_workers.retain(|id| id != worker_id);
                self.master_wake
                    .newly_idle_workers
                    .retain(|id| id != worker_id);
            }
            Event::ResponsibilityFenceSet { fence } => {
                // `closing` is the only phase B23-F writes. `partial` and
                // `unknown` are accepted so a later lifecycle owner can record
                // an unresolved close phase that must stay fenced across
                // replay; none of them release the fence.
                if !fence.is_active() {
                    return Err(format!(
                        "responsibility fence {} has invalid initial state {}",
                        fence.operation_id, fence.state
                    ));
                }
                if fence.worker_id.trim().is_empty()
                    || fence.project_scope.trim().is_empty()
                    || fence.app_scope.trim().is_empty()
                    || fence.binding_id.trim().is_empty()
                    || fence.endpoint_generation == 0
                {
                    return Err(format!(
                        "responsibility fence {} has incomplete exact target identity",
                        fence.operation_id
                    ));
                }
                self.responsibility_fences
                    .insert(fence.operation_id.clone(), fence.clone());
            }
            Event::PeerLifecycleOperationRecorded { operation } => {
                if operation.operation_id.trim().is_empty()
                    || operation.actor_id.trim().is_empty()
                    || operation.project_scope.as_str().trim().is_empty()
                    || operation.app_scope_id.as_str().trim().is_empty()
                    || operation.target.as_ref().is_some_and(|target| {
                        target.worker_id.trim().is_empty()
                            || target.binding_id.as_str().trim().is_empty()
                            || target.endpoint_generation == 0
                            || target.project_scope != operation.project_scope
                            || target.app_scope_id != operation.app_scope_id
                    })
                    || (operation.action != PeerLifecycleAction::Create
                        && operation.target.is_none())
                {
                    return Err(format!(
                        "peer lifecycle operation {} has incomplete exact target identity",
                        operation.operation_id
                    ));
                }
                if let Some(existing) = self.peer_lifecycle_operations.get(&operation.operation_id)
                {
                    if existing.action != operation.action
                        || existing.actor_id != operation.actor_id
                        || existing.project_scope != operation.project_scope
                        || existing.app_scope_id != operation.app_scope_id
                        || (existing.target.is_some() && existing.target != operation.target)
                        || (existing.target.is_none()
                            && operation.target.is_some()
                            && operation.action != PeerLifecycleAction::Create)
                        || existing.intent_digest != operation.intent_digest
                        || existing.query_capability_hash != operation.query_capability_hash
                        || existing.created_ms != operation.created_ms
                    {
                        return Err(format!(
                            "peer lifecycle operation {} changed immutable intent",
                            operation.operation_id
                        ));
                    }
                    if existing.responsibility_snapshot.is_some()
                        && operation.responsibility_snapshot.is_none()
                    {
                        return Err(format!(
                            "peer lifecycle operation {} dropped its responsibility snapshot",
                            operation.operation_id
                        ));
                    }
                }
                if operation.action == PeerLifecycleAction::Create {
                    let Some(create) = operation.create.as_ref() else {
                        return Err("Create requires a frozen intent receipt".into());
                    };
                    crate::identity::validate_id_for_protocol(&create.peer_id)
                        .map_err(|error| error.to_string())?;
                    if create.cwd.is_empty()
                        || operation.target.as_ref().is_some_and(|target| {
                            target.worker_id != create.peer_id
                                || target.transport.thread_id != create.thread_id
                        })
                        || (operation.phase == crate::proto::PeerLifecyclePhase::Complete
                            && (operation.target.is_none()
                                || create.thread_id.is_none()
                                || create.readiness.state
                                    != crate::proto::PeerLifecycleStage::Verified))
                    {
                        return Err("Create has inconsistent exact receipt".into());
                    }
                    if let Some(existing) =
                        self.peer_lifecycle_operations.get(&operation.operation_id)
                    {
                        let old = existing.create.as_ref().ok_or("Create lost its intent")?;
                        if old.peer_id != create.peer_id
                            || old.cwd != create.cwd
                            || old.model != create.model
                            || (old.thread_id.is_some() && old.thread_id != create.thread_id)
                            || (existing.phase != crate::proto::PeerLifecyclePhase::IntentPersisted
                                && matches!(
                                    operation.phase,
                                    crate::proto::PeerLifecyclePhase::IntentPersisted
                                        | crate::proto::PeerLifecyclePhase::HostDispatchClaimed
                                )
                                && existing.phase != operation.phase)
                        {
                            return Err(
                                "Create changed frozen intent or consumed dispatch claim".into()
                            );
                        }
                    } else if self.peer_lifecycle_operations.values().any(|other| {
                        other.action == PeerLifecycleAction::Create
                            && other.project_scope == operation.project_scope
                            && other.app_scope_id == operation.app_scope_id
                            && other
                                .create
                                .as_ref()
                                .is_some_and(|receipt| receipt.peer_id == create.peer_id)
                            && other.phase != crate::proto::PeerLifecyclePhase::Complete
                            && other.phase != crate::proto::PeerLifecyclePhase::Refused
                    }) {
                        return Err("Create peer id is reserved by an unresolved operation".into());
                    }
                } else if operation.create.is_some() {
                    return Err("non-Create operation contains Create receipt".into());
                }
                if operation.action == PeerLifecycleAction::Close {
                    if let Some(fence) = crate::server::state::close_fence_for_operation(&operation)
                    {
                        self.responsibility_fences
                            .insert(fence.operation_id.clone(), fence);
                    } else {
                        self.responsibility_fences.remove(&operation.operation_id);
                    }
                }
                self.peer_lifecycle_operations
                    .insert(operation.operation_id.clone(), operation.clone());
            }
            Event::LegacyMasterTransferred { .. } => {}
            Event::Sent { msg } => {
                self.msgs.insert(msg.id.clone(), msg.clone());
            }
            Event::ReplyObligationSet { obligation } => {
                self.reply_obligations
                    .insert(obligation.message_id.clone(), obligation.clone());
            }
            Event::ReplyObligationCleared { message_id } => {
                self.reply_obligations.remove(message_id);
            }
            Event::ReplyObligationReminded { message_id, reminded } => {
                if let Some(obligation) = self.reply_obligations.get_mut(message_id) {
                    obligation.reminded_idle_episode = *reminded;
                }
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
                let accepted_goal_deadline_revision =
                    self.msgs.get(message_id).is_some_and(|message| {
                        message.from == "collab-server"
                            && self
                                .wake_bindings
                                .get(message_id)
                                .and_then(|subscription_id| {
                                    self.notification_subscriptions.get(subscription_id)
                                })
                                .is_some_and(|subscription| {
                                    is_goal_deadline(subscription)
                                        && subscription_affects_master_wake(self, subscription)
                                        && message.to == subscription.worker_id
                                        && self.master_wake.active_goal_revision.is_some_and(
                                            |revision| {
                                                subscription_goal_deadline_revision(subscription)
                                                    == Some(revision)
                                            },
                                        )
                                })
                    });
                if accepted_goal_deadline_revision {
                    crate::server::notification_state::clear_delivered_master_goal_due(
                        &mut self.master_wake,
                    );
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
                    crate::server::notification_state::mark_master_wake_skipped_busy(
                        &mut self.master_wake,
                    );
                }
                if is_goal_deadline(subscription) {
                    if affects_master_wake {
                        let revision = u64::try_from(*due_ms).unwrap_or(0);
                        crate::server::notification_state::accumulate_master_wake(
                            &mut self.master_wake,
                            &MasterWakeSignal::GoalDue { revision },
                            *skipped_ms,
                        );
                        crate::server::notification_state::mark_master_wake_skipped_busy(
                            &mut self.master_wake,
                        );
                    }
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
                    crate::server::notification_state::mark_master_wake_delivered(
                        &mut self.master_wake,
                    );
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
            Event::BoardDetailsChanged { task_id, details } => {
                self.board_details.insert(task_id.clone(), details.clone());
            }
            Event::BoardPatrolCancelled { record } => {
                self.cancelled_board_patrol_tasks
                    .insert(record.task_id.clone(), record.clone());
            }
            Event::BoardPatrolRearmed { task_id, owner_key } => {
                if self
                    .cancelled_board_patrol_tasks
                    .get(task_id)
                    .is_some_and(|record| record.owner_key == *owner_key)
                {
                    self.cancelled_board_patrol_tasks.remove(task_id);
                }
            }
            Event::BoardPatrolCancellationCleared { task_id } => {
                self.cancelled_board_patrol_tasks.remove(task_id);
            }
            Event::BoardPatrolGateUpdated { worker_id, gate } => {
                self.board_patrol_gates
                    .insert(worker_id.clone(), gate.clone());
            }
            Event::ReducerSnapshot { .. } => {
                return Err("reducer snapshot is replay-only".into());
            }
            Event::TaskCreated { task } | Event::TaskUpdated { task } => {
                let cancellation_owner = self
                    .cancelled_board_patrol_tasks
                    .get(&task.id)
                    .map(|record| record.owner_key.clone());
                if task_status_is_terminal(&task.status) {
                    self.cancelled_board_patrol_tasks.remove(&task.id);
                } else if let Some(record_owner) = cancellation_owner {
                    let current_owner = self
                        .task_assignments
                        .get(&task.id)
                        .map(|assignment| assignment.name.clone())
                        .unwrap_or_else(|| board_patrol_owner_key(self, &task.owner));
                    if record_owner != current_owner {
                        self.cancelled_board_patrol_tasks.remove(&task.id);
                    }
                }
                let known_public = self.workers.contains_key(&task.owner)
                    && !self
                        .subagents
                        .values()
                        .any(|child| child.peer == task.owner);
                match self.board_details.get_mut(&task.id) {
                    Some(details) => details.revision += 1,
                    None => {
                        self.board_details.insert(
                            task.id.clone(),
                            crate::board::BoardTaskDetails::legacy(&task.id, known_public),
                        );
                    }
                }
                self.tasks.insert(task.id.clone(), task.clone());
            }
            Event::NameResolutionCommitted {
                project_scope,
                occupancy,
                assignments,
            } => {
                self.apply_name_resolution(project_scope, occupancy, assignments)?;
                self.rebind_reply_obligations_for_name(
                    project_scope,
                    &occupancy.name,
                    &occupancy.worker_id,
                );
            }
            Event::NameOccupancyRevisionRetained {
                project_scope,
                name,
                revision,
            } => {
                self.retain_name_occupancy_revision(project_scope, name, *revision);
            }
            Event::TaskAssignmentUpdated { assignment } => {
                self.apply_task_assignment(assignment)?;
                if self
                    .cancelled_board_patrol_tasks
                    .get(&assignment.task_id)
                    .is_some_and(|record| record.owner_key != assignment.name)
                {
                    self.cancelled_board_patrol_tasks
                        .remove(&assignment.task_id);
                }
            }
            Event::NameWorkRolesUpdated {
                project_scope,
                record,
            } => {
                self.name_work_roles
                    .entry(project_scope.clone())
                    .or_default()
                    .insert(record.name.clone(), record.clone());
            }
            Event::WorkflowTemplateUpdated {
                project_scope,
                target,
                entry,
            } => {
                if let Some(entry) = entry {
                    self.workflow_templates
                        .entry(project_scope.clone())
                        .or_default()
                        .insert(target.clone(), entry.clone());
                } else {
                    let empty = match self.workflow_templates.get_mut(project_scope) {
                        Some(targets) => {
                            targets.remove(target);
                            targets.is_empty()
                        }
                        None => false,
                    };
                    if empty {
                        self.workflow_templates.remove(project_scope);
                    }
                }
            }
            Event::ProjectWorkPolicyUpdated { work_kind, policy } => {
                self.project_work_policy
                    .insert(work_kind.clone(), policy.clone());
            }
            Event::NameRecoveryUpdated { recovery } => {
                self.apply_name_recovery(recovery)?;
            }
            Event::AgentPresenceObserved { presence } => {
                self.apply_agent_presence(presence)?;
            }
            Event::AgentActivityObserved { activity } => {
                self.apply_agent_activity(activity)?;
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
                if let Some(details) = self.board_details.get_mut(task_id) {
                    details.revision += 1;
                }
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
                if let Some(details) = self.board_details.get_mut(&receipt.task_id) {
                    details.revision += 1;
                }
                self.cleanup_receipts
                    .insert(receipt.task_id.clone(), receipt.clone());
            }
            Event::MigrationUpdated { migration } => {
                self.migration = Some(migration.clone());
            }
            Event::CeoGrantSet { grant } => {
                self.ceo_grant = grant.clone();
            }
            Event::CeoBootstrapSet { record } => {
                self.ceo_bootstraps
                    .insert(record.operation_id.clone(), record.clone());
            }
            Event::CeoProgressRecorded {
                project,
                content,
                urgent,
            } => {
                self.ceo_report_state
                    .record_progress(project.clone(), content.clone(), *urgent);
            }
            Event::CeoReportBatchPrepared {
                batch_id,
                coverage,
                urgent,
            } => {
                self.ceo_report_state
                    .prepare_batch(batch_id.clone(), coverage.clone(), *urgent)
                    .map_err(|error| error.to_string())?;
            }
            Event::CeoReportBatchAcknowledged {
                batch_id,
                reported_ms,
            } => {
                self.ceo_report_state
                    .acknowledge_batch(batch_id, *reported_ms)
                    .map_err(|error| error.to_string())?;
            }
            Event::CeoReportStateRestored { report_state } => {
                self.ceo_report_state = report_state.clone();
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
            Event::GlobalBootstrapPolicySet { policy } => {
                let project = self
                    .global
                    .projects
                    .entry(policy.project_scope.as_str().to_owned())
                    .or_insert(
                        crate::server::global_state::ProjectState::new(
                            policy.project_scope.clone(),
                        )
                        .map_err(|error| error.to_string())?,
                    );
                project
                    .bootstrap_policies
                    .insert(policy.app_scope_id.as_str().to_owned(), policy.clone());
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
                let failed_is_local_route = failed
                    .session_id
                    .as_ref()
                    .zip(failed.native_thread_id.as_ref())
                    .is_some_and(|(session, thread)| {
                        next.lookup_current_thread_route(session, thread) == Some(failed)
                    });
                if failed_is_local_route {
                    next.retire_current_thread_route(failed.clone())
                        .map_err(|error| {
                            format!("global reducer rejected route rollback: {error}")
                        })?;
                    if let Some(previous) = previous.as_ref() {
                        next.set_current_thread_route(previous.clone())
                            .map_err(|error| {
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
                if binding.session_id.is_none() && binding.tmux_endpoint.is_none() {
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
                self.apply_global_event(&GlobalEvent::RuntimeBindingLedgerClassified {
                    record: record.clone(),
                })?;
            }
            Event::GlobalLedgerScanReceiptRecorded { receipt } => {
                self.apply_global_event(&GlobalEvent::LedgerScanReceiptRecorded {
                    receipt: receipt.clone(),
                })?;
            }
            Event::GlobalMasterGrantReplacementStarted { intent } => {
                self.apply_global_event(&GlobalEvent::MasterGrantReplacementStarted {
                    intent: intent.clone(),
                })?;
            }
            Event::GlobalMasterGrantReplacementCompleted { receipt } => {
                self.apply_global_event(&GlobalEvent::MasterGrantReplacementCompleted {
                    receipt: receipt.clone(),
                })?;
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
        self.mailbox_name_addresses.remove(id);
        self.mailbox_handoffs
            .retain(|handoff| handoff.message_id != id);
        self.notification_delivery_failures.remove(id);
        self.notification_delivery_accepted.remove(id);
        self.notification_delivery_evidence.remove(id);
        self.delivery_modes.remove(id);
        self.delivery_source_threads.remove(id);
        self.wake_bindings.remove(id);
    }

    /// Persisted work specialities for one stable name. Session carriers and
    /// transport bindings are deliberately not consulted.
    pub fn name_work_roles(&self, project_scope: &str, name: &str) -> Vec<String> {
        self.name_work_roles
            .get(project_scope)
            .and_then(|names| names.get(name))
            .map(|record| record.work_roles.clone())
            .unwrap_or_default()
    }

    /// Materialize the effective policy for a task that is being published or
    /// explicitly re-applied. Later changes to broader defaults must not mutate
    /// this resolved contract.
    pub fn materialize_work_policy(
        &self,
        work_kind: Option<&str>,
        parent_task_id: Option<&str>,
        explicit: &crate::board::WorkPolicy,
    ) -> crate::board::WorkPolicy {
        let work_kind = work_kind.or_else(|| parent_task_id
            .and_then(|parent|self.board_details.get(parent))
            .and_then(|details|details.work_policy.work_kind.as_deref()));
        let mut layers = Vec::new();
        if let Some(kind) = work_kind {
            if let Some(policy) = self.project_work_policy.get(kind) {
                layers.push(policy.clone());
            }
        }
        if let Some(parent) = parent_task_id {
            if let Some(details) = self.board_details.get(parent) {
                if details.work_policy.work_kind.as_deref() == work_kind {
                    layers.push(details.work_policy.clone());
                }
                if let Some(policy) = work_kind.and_then(|kind|details.child_work_policies.get(kind)) {
                    layers.push(policy.clone());
                }
            }
        }
        layers.push(explicit.clone());
        let mut effective = crate::board::WorkPolicy::default();
        for layer in layers {
            crate::server::overlay_work_policy(&mut effective,&layer);
        }
        if effective.work_kind.is_none() {
            effective.work_kind = work_kind.map(str::to_owned);
        }
        effective
    }

    /// Task contract has already been materialized; read it without resolving
    /// broader defaults again.
    pub fn effective_work_policy(&self, task_id: &str) -> crate::board::WorkPolicy {
        self.board_details
            .get(task_id)
            .map(|details| details.work_policy.clone())
            .unwrap_or_default()
    }

    /// One completion projection shared by read-only views and lifecycle write
    /// gates. Legacy tasks with no review-subject revision keep the previous
    /// evidence-based acceptance behavior.
    pub fn delivery_completion_projection(
        &self,
        task_id: &str,
    ) -> (bool, Vec<String>) {
        let Some(task) = self.tasks.get(task_id) else {
            return (false, vec!["task".into()]);
        };
        let mut missing = Vec::new();
        let Some(lifecycle) = self.task_lifecycle.get(task_id) else {
            return (false, vec!["review".into()]);
        };
        if !crate::server::work_review_valid(self,task_id) {
            missing.push("review".into());
        }
        if lifecycle.delivered_ms.is_none() || lifecycle.delivery_evidence.is_none() {
            missing.push("delivery".into());
        }
        if lifecycle.delivery_kind == TaskDeliveryKind::Code
            && lifecycle.integration_commit.is_none()
        {
            missing.push("integration".into());
        }
        let cleanup_ok = (task.worktree_path.is_none() && task.branch.is_none())
            || self
                .cleanup_receipts
                .get(task_id)
                .is_some_and(|receipt| receipt.verification == CleanupVerification::Verified);
        if !cleanup_ok {
            missing.push("cleanup".into());
        }
        if task.status != "closed" {
            missing.push("close".into());
        }
        if self.cleanup_receipts.get(task_id).is_some_and(|receipt| receipt.manual_reason.is_some()) {
            missing.push("manual_termination".into());
        }
        (missing.is_empty(), missing)
    }

    pub fn snapshot_events(&self) -> Vec<Event> {
        let (sequence, revision, mut events) = self.snapshot_contents();
        events.push(Event::ReducerCheckpoint { sequence, revision });
        events
    }

    /// Produce the restoration payload for a compacted journal baseline.
    /// The caller stores the counters in the replay-only envelope.
    pub fn snapshot_contents(&self) -> (u64, u64, Vec<Event>) {
        let mut events = Vec::new();
        // History precedes current mailbox rows so restoration does not repeat
        // a handoff or reset a later consumption or wake.
        events.extend(
            self.mailbox_handoffs
                .iter()
                .cloned()
                .map(|handoff| Event::MailboxReassigned { handoff }),
        );
        for (project_scope, clear_local) in &self.pending_project_resets {
            events.push(Event::ProjectResetStarted {
                project_scope: project_scope.clone(),
                clear_local: *clear_local,
                worker_ids: Vec::new(),
            });
        }
        if master_wake_snapshot_required(&self.master_wake) {
            events.push(Event::MasterWakeUpdated {
                accumulator: self.master_wake.clone(),
            });
        }
        let mut fences: Vec<_> = self.responsibility_fences.values().cloned().collect();
        fences.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        events.extend(
            fences
                .into_iter()
                .map(|fence| Event::ResponsibilityFenceSet { fence }),
        );
        let mut operations: Vec<_> = self.peer_lifecycle_operations.values().cloned().collect();
        operations.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        events.extend(
            operations
                .into_iter()
                .map(|operation| Event::PeerLifecycleOperationRecorded { operation }),
        );
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
        let mut names: Vec<_> = self.worker_names.iter().collect();
        names.sort_by(|left, right| left.0.cmp(right.0));
        events.extend(names.into_iter().map(|(id, name)| Event::WorkerNamed {
            worker_id: id.clone(),
            name: Some(name.clone()),
        }));
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
        let mut revisions: Vec<_> = self
            .name_occupancy_revisions
            .iter()
            .flat_map(|(project, names)| {
                names
                    .iter()
                    .map(move |(name, revision)| (project.clone(), name.clone(), *revision))
            })
            .collect();
        revisions.sort();
        events.extend(
            revisions
                .into_iter()
                .map(
                    |(project_scope, name, revision)| Event::NameOccupancyRevisionRetained {
                        project_scope,
                        name,
                        revision,
                    },
                ),
        );
        let mut occupancies: Vec<_> = self
            .name_occupancies
            .iter()
            .flat_map(|(project_scope, names)| {
                names
                    .values()
                    .map(move |occupancy| (project_scope.clone(), occupancy.clone()))
            })
            .collect();
        occupancies.sort_by(|(left_scope, left), (right_scope, right)| {
            (left_scope.as_str(), left.name.as_str())
                .cmp(&(right_scope.as_str(), right.name.as_str()))
        });
        for (project_scope, occupancy) in occupancies {
            let mut assignments: Vec<_> = self
                .task_assignments
                .values()
                .filter(|assignment| {
                    assignment.name == occupancy.name
                        && assignment.session_id == occupancy.session_id
                })
                .cloned()
                .collect();
            assignments.sort_by(|left, right| left.task_id.cmp(&right.task_id));
            events.push(Event::NameResolutionCommitted {
                project_scope,
                occupancy,
                assignments,
            });
        }
        let mut assignments: Vec<_> = self.task_assignments.values().cloned().collect();
        assignments.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        events.extend(
            assignments
                .into_iter()
                .map(|assignment| Event::TaskAssignmentUpdated { assignment }),
        );
        let mut role_records: Vec<_> = self
            .name_work_roles
            .iter()
            .flat_map(|(project_scope, names)| {
                names
                    .values()
                    .map(move |record| (project_scope.clone(), record.clone()))
            })
            .collect();
        role_records.sort_by(|(left_scope, left), (right_scope, right)| {
            (left_scope.as_str(), left.name.as_str()).cmp(&(right_scope.as_str(), right.name.as_str()))
        });
        events.extend(role_records.into_iter().map(|(project_scope, record)| {
            Event::NameWorkRolesUpdated {
                project_scope,
                record,
            }
        }));
        let mut template_records: Vec<_> = self
            .workflow_templates
            .iter()
            .flat_map(|(project_scope, targets)| {
                targets
                    .iter()
                    .map(move |(target, entry)| {
                        (project_scope.clone(), target.clone(), entry.clone())
                    })
            })
            .collect();
        template_records.sort_by(|(left_scope, left_target, _), (right_scope, right_target, _)| {
            (left_scope.as_str(), left_target.as_str())
                .cmp(&(right_scope.as_str(), right_target.as_str()))
        });
        events.extend(
            template_records
                .into_iter()
                .map(|(project_scope, target, entry)| Event::WorkflowTemplateUpdated {
                    project_scope,
                    target,
                    entry: Some(entry),
                }),
        );
        let mut project_policies: Vec<_> = self.project_work_policy.iter().collect();
        project_policies.sort_by(|left, right| left.0.cmp(right.0));
        events.extend(
            project_policies
                .into_iter()
                .map(|(work_kind, policy)| Event::ProjectWorkPolicyUpdated {
                    work_kind: work_kind.clone(),
                    policy: policy.clone(),
                }),
        );
        let mut recoveries: Vec<_> = self.name_recoveries.values().cloned().collect();
        recoveries.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        events.extend(
            recoveries
                .into_iter()
                .map(|recovery| Event::NameRecoveryUpdated { recovery }),
        );
        let mut presences: Vec<_> = self
            .agent_presence
            .iter()
            .flat_map(|(project_scope, workers)| {
                workers
                    .values()
                    .map(move |presence| (project_scope.clone(), presence.clone()))
            })
            .collect();
        presences.sort_by(|(left_scope, left), (right_scope, right)| {
            (left_scope.as_str(), left.worker_id.as_str())
                .cmp(&(right_scope.as_str(), right.worker_id.as_str()))
        });
        events.extend(
            presences
                .into_iter()
                .map(|(_, presence)| Event::AgentPresenceObserved { presence }),
        );
        let mut activities: Vec<_> = self
            .agent_activity
            .iter()
            .flat_map(|(project_scope, workers)| {
                workers
                    .values()
                    .map(move |activity| (project_scope.clone(), activity.clone()))
            })
            .collect();
        activities.sort_by(|(left_scope, left), (right_scope, right)| {
            (left_scope.as_str(), left.worker_id.as_str())
                .cmp(&(right_scope.as_str(), right.worker_id.as_str()))
        });
        events.extend(
            activities
                .into_iter()
                .map(|(_, activity)| Event::AgentActivityObserved { activity }),
        );
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
            if let Some(address) = self.mailbox_name_addresses.get(&id) {
                events.push(Event::MailboxNameAddressed {
                    message_id: id.clone(),
                    address: address.clone(),
                });
            }
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
        // Unsettled obligations survive compaction. They follow the message
        // rows above so a restored journal has the original message first.
        let mut obligations: Vec<_> = self.reply_obligations.values().cloned().collect();
        obligations.sort_by(|a, b| {
            (a.created_ms, a.message_id.as_str()).cmp(&(b.created_ms, b.message_id.as_str()))
        });
        events.extend(
            obligations
                .into_iter()
                .map(|obligation| Event::ReplyObligationSet { obligation }),
        );
        if let Some(migration) = self.migration.clone() {
            events.push(Event::MigrationUpdated { migration });
        }
        if let Some(grant) = self.ceo_grant.clone() {
            events.push(Event::CeoGrantSet {
                grant: Some(grant),
            });
        }
        let mut bootstraps: Vec<_> = self.ceo_bootstraps.values().cloned().collect();
        bootstraps.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        events.extend(
            bootstraps
                .into_iter()
                .map(|record| Event::CeoBootstrapSet { record }),
        );
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
            for policy in project.bootstrap_policies.values() {
                events.push(Event::GlobalBootstrapPolicySet {
                    policy: policy.clone(),
                });
            }
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
        for intent in self.global.master_grant_replacement_intents.values() {
            events.push(Event::GlobalMasterGrantReplacementStarted {
                intent: intent.clone(),
            });
        }
        for receipt in self.global.master_grant_replacement_receipts.values() {
            events.push(Event::GlobalMasterGrantReplacementCompleted {
                receipt: receipt.clone(),
            });
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
        // Restore exact final descriptive records after replaying the resource
        // snapshots above. Snapshot TaskCreated is not an additional update.
        let mut details: Vec<_> = self.board_details.iter().collect();
        details.sort_by(|left, right| left.0.cmp(right.0));
        events.extend(
            details
                .into_iter()
                .map(|(task_id, details)| Event::BoardDetailsChanged {
                    task_id: task_id.clone(),
                    details: details.clone(),
                }),
        );
        events.extend(
            self.cancelled_board_patrol_tasks
                .values()
                .cloned()
                .map(|record| Event::BoardPatrolCancelled { record }),
        );
        events.extend(self.board_patrol_gates.iter().map(|(worker_id, gate)| {
            Event::BoardPatrolGateUpdated {
                worker_id: worker_id.clone(),
                gate: gate.clone(),
            }
        }));
        // Carry the canonical CEO report state only when it is meaningful: an
        // existing CEO grant, a pending revision, an unacknowledged batch, or a
        // recorded actual-report time. This preserves cleared-grant outstanding
        // progress and report history while avoiding an unused empty CEO state
        // on every non-CEO project. The restore assigns the exact helper state.
        let ceo_meaningful = self.ceo_grant.is_some()
            || !self.ceo_report_state.pending_revisions().is_empty()
            || self.ceo_report_state.batch_count() > 0
            || self.ceo_report_state.last_actual_report_ms().is_some();
        if ceo_meaningful {
            events.push(Event::CeoReportStateRestored {
                report_state: self.ceo_report_state.clone(),
            });
        }
        (self.sequence, self.revision, events)
    }

    pub fn admission_frozen(&self) -> bool {
        self.journal_poison.is_some()
            || self
                .pending_project_resets
                .values()
                .any(|clear_local| *clear_local)
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

#[cfg(test)]
mod ceo_reply_scope_tests {
    use super::*;

    #[test]
    fn cross_project_name_handoff_does_not_rebind_foreign_sender() {
        let mut state=State::default();
        state.reply_obligations.insert("request".into(), crate::server::state::ReplyObligation {
            message_id:"request".into(),project:"/target".into(),from_project:Some("/ceo".into()),from_ceo:true,
            from_worker_id:"ceo-worker".into(),from_name:Some("SameName".into()),
            recipient_worker_id:"old-master".into(),recipient_name:Some("SameName".into()),
            policy:crate::proto::ReplyPolicy::Required,created_ms:1,reminded_idle_episode:true,
        });
        state.rebind_reply_obligations_for_name("/target","SameName","new-master");
        let obligation=&state.reply_obligations["request"];
        assert_eq!(obligation.from_worker_id,"ceo-worker","a recipient-project name cannot own the foreign sender");
        assert_eq!(obligation.recipient_worker_id,"new-master");
        assert!(!obligation.reminded_idle_episode);
    }
}

#[cfg(test)]
mod board_patrol_state_tests {
    use super::*;

    fn cancel_record(
        task_id: &str,
        owner_key: &str,
    ) -> crate::board_patrol_policy::TaskPatrolCancelRecord {
        crate::board_patrol_policy::TaskPatrolCancelRecord {
            task_id: task_id.into(),
            owner_key: owner_key.into(),
            reason: Some("operator pause".into()),
            resume_condition: Some("operator resume".into()),
            cancelled_ms: 42,
        }
    }

    fn gate(owner_key: &str) -> crate::board_patrol_policy::PeerBoardPatrolGate {
        crate::board_patrol_policy::PeerBoardPatrolGate {
            owner_key: owner_key.into(),
            last_wake_ms: 100,
            last_realtime_idle_since_ms: 90,
            last_wake_tasks: vec!["task-a".into()],
            next_due_ms: 900_100,
        }
    }

    fn unrelated_task() -> TaskRec {
        TaskRec {
            id: "task-unrelated".into(),
            owner: "owner-b".into(),
            created_by: "master".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "working".into(),
            next_step: None,
            wait: None,
            created_ms: 1,
            updated_ms: 1,
        }
    }

    fn unrelated_message() -> Message {
        Message {
            id: "message-unrelated".into(),
            from: "owner-a".into(),
            to: "owner-b".into(),
            mtype: "request".into(),
            subject: None,
            body: "keep me".into(),
            in_reply_to: None,
            created_ms: 1,
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        }
    }

    fn replay_through_serde(state: &State) -> State {
        let body = state
            .snapshot_events()
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>();
        let mut replayed = State::default();
        for line in body {
            let event: Event = serde_json::from_str(&line).unwrap();
            replayed.apply(&event);
        }
        replayed
    }

    #[test]
    fn cancellation_and_gate_survive_snapshot_serde_replay_preserving_unrelated_state() {
        let mut state = State::default();
        state.apply(&Event::TaskCreated {
            task: unrelated_task(),
        });
        state.apply(&Event::Sent {
            msg: unrelated_message(),
        });
        state.apply(&Event::BoardPatrolCancelled {
            record: cancel_record("task-a", "owner-a"),
        });
        state.apply(&Event::BoardPatrolGateUpdated {
            worker_id: "worker-a".into(),
            gate: gate("owner-a"),
        });

        let replayed = replay_through_serde(&state);

        assert_eq!(
            replayed.cancelled_board_patrol_tasks.get("task-a"),
            state.cancelled_board_patrol_tasks.get("task-a")
        );
        assert_eq!(
            replayed.board_patrol_gates.get("worker-a"),
            state.board_patrol_gates.get("worker-a")
        );
        assert_eq!(replayed.tasks["task-unrelated"].id, "task-unrelated");
        assert_eq!(replayed.msgs["message-unrelated"].body, "keep me");
    }

    #[test]
    fn rearm_removes_only_matching_owner() {
        let mut state = State::default();
        state.apply(&Event::BoardPatrolCancelled {
            record: cancel_record("task-a", "owner-a"),
        });

        state.apply(&Event::BoardPatrolRearmed {
            task_id: "task-a".into(),
            owner_key: "owner-b".into(),
        });
        assert!(state.cancelled_board_patrol_tasks.contains_key("task-a"));

        state.apply(&Event::BoardPatrolRearmed {
            task_id: "task-a".into(),
            owner_key: "owner-a".into(),
        });
        assert!(!state.cancelled_board_patrol_tasks.contains_key("task-a"));
    }

    #[test]
    fn explicit_clear_persists_absence_after_replay() {
        let mut state = State::default();
        state.apply(&Event::BoardPatrolCancelled {
            record: cancel_record("task-a", "owner-a"),
        });
        state.apply(&Event::BoardPatrolCancellationCleared {
            task_id: "task-a".into(),
        });

        let replayed = replay_through_serde(&state);
        assert!(replayed.cancelled_board_patrol_tasks.is_empty());
    }
}

#[cfg(test)]
mod ceo_report_state_wiring_tests {
    use super::*;
    use crate::ceo_report_state::CeoProgressContent;
    use std::collections::BTreeMap;

    fn replay_through_serde(state: &State) -> State {
        let body = state
            .snapshot_events()
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>();
        let mut replayed = State::default();
        for line in body {
            let event: Event = serde_json::from_str(&line).unwrap();
            replayed.apply(&event);
        }
        replayed
    }

    fn content(subject: &str, body: &str) -> CeoProgressContent {
        CeoProgressContent::new(subject, body)
    }

    fn coverage(project: &str, revision: u64) -> BTreeMap<String, u64> {
        BTreeMap::from([(project.to_owned(), revision)])
    }

    fn progress(project: &str, body: &str) -> Event {
        Event::CeoProgressRecorded {
            project: project.into(),
            content: content("progress", body),
            urgent: false,
        }
    }

    fn prepared(batch_id: &str, project: &str, revision: u64) -> Event {
        Event::CeoReportBatchPrepared {
            batch_id: batch_id.into(),
            coverage: coverage(project, revision),
            urgent: false,
        }
    }

    fn acknowledged(batch_id: &str, reported_ms: u64) -> Event {
        Event::CeoReportBatchAcknowledged {
            batch_id: batch_id.into(),
            reported_ms,
        }
    }

    fn contains_restored(events: &[Event]) -> bool {
        events
            .iter()
            .any(|event| matches!(event, Event::CeoReportStateRestored { .. }))
    }

    #[test]
    fn progress_and_prepare_survive_snapshot_serde_replay_without_ack() {
        let mut state = State::default();
        state.apply(&progress("proj-a", "first"));
        state.apply(&prepared("batch-1", "proj-a", 1));

        assert!(contains_restored(&state.snapshot_events()));

        let replayed = replay_through_serde(&state);

        let pending = replayed
            .ceo_report_state
            .pending_revision("proj-a")
            .expect("pending revision survives replay");
        assert_eq!(pending.revision, 1);
        assert_eq!(pending.content, content("progress", "first"));
        assert_eq!(replayed.ceo_report_state.batch_count(), 1);
        assert_eq!(
            replayed.ceo_report_state.batches["batch-1"].coverage,
            coverage("proj-a", 1)
        );
        assert_eq!(replayed.ceo_report_state.last_actual_report_ms(), None);
        assert_eq!(
            replayed.ceo_report_state.projects["proj-a"].reported_revision,
            0
        );
    }

    #[test]
    fn ack_of_superseded_revision_keeps_later_pending_and_replay_time_then_rejects_ack() {
        let mut state = State::default();
        state.apply(&progress("proj-a", "revision one"));
        state.apply(&prepared("batch-1", "proj-a", 1));
        // A newer same-project revision exists before the acknowledgement.
        state.apply(&progress("proj-a", "revision two"));
        state.apply(&acknowledged("batch-1", 1_000));

        assert_eq!(state.ceo_report_state.last_actual_report_ms(), Some(1_000));
        assert_eq!(
            state
                .ceo_report_state
                .pending_revision("proj-a")
                .expect("revision two stays pending live")
                .revision,
            2
        );

        let mut replayed = replay_through_serde(&state);
        assert_eq!(
            replayed
                .ceo_report_state
                .pending_revision("proj-a")
                .expect("revision two stays pending after replay")
                .revision,
            2
        );
        assert_eq!(
            replayed.ceo_report_state.last_actual_report_ms(),
            Some(1_000)
        );

        // Unknown and repeated acknowledgements are explicit errors and do not
        // advance the declared report time or change pending progress.
        assert!(replayed
            .apply_checked(&acknowledged("missing-batch", 9_000))
            .is_err());
        assert_eq!(
            replayed.ceo_report_state.last_actual_report_ms(),
            Some(1_000)
        );
        assert!(replayed
            .apply_checked(&acknowledged("batch-1", 9_000))
            .is_err());
        assert_eq!(
            replayed.ceo_report_state.last_actual_report_ms(),
            Some(1_000)
        );
        assert_eq!(
            replayed
                .ceo_report_state
                .pending_revision("proj-a")
                .expect("revision two still pending")
                .revision,
            2
        );
    }

    #[test]
    fn default_state_emits_no_empty_ceo_event_but_nonempty_report_state_does() {
        let untouched = State::default();
        assert!(!contains_restored(&untouched.snapshot_events()));

        let mut cleared_grant = State::default();
        cleared_grant.apply(&Event::CeoGrantSet {
            grant: Some(crate::ceo::CeoGrant {
                workspace: "/workspace".into(),
                app_scope_id: "app".into(),
                name: "CEO".into(),
                granted_by: "user".into(),
                approval: "approved".into(),
                granted_ms: 1,
            }),
        });
        cleared_grant.apply(&Event::CeoGrantSet { grant: None });
        assert!(cleared_grant.ceo_grant.is_none());

        cleared_grant.apply(&progress("project-after-revoke", "retained"));
        let events = cleared_grant.snapshot_events();
        let restored = events
            .iter()
            .find_map(|event| match event {
                Event::CeoReportStateRestored { report_state } => Some(report_state),
                _ => None,
            })
            .expect("nonempty report state is retained without a CEO grant");
        assert!(restored.pending_revision("project-after-revoke").is_some());
    }
}
