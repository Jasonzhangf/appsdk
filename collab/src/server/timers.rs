use crate::proto::SelectedTransport;
use crate::server::global_state::{
    LedgerScanReceipt, RuntimeBindingLedgerRecord, RuntimeBindingLedgerState,
};
use crate::server::state::{
    goal_deadline_key, is_goal_deadline, now_ms, Event, Message, MAX_WAKE_ATTEMPTS,
};
use crate::server::Server;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Server-side scheduler for finite subscriptions and bounded waits. It never
/// creates task continuations or infers that ordinary work needs a wake.
pub fn tick(server: &Arc<Server>) {
    tick_at(server, now_ms());
}

fn tick_at(server: &Arc<Server>, now: i64) {
    super::keepalive::tick_at(server, now);
    super::purge_expired_storage(server, now);
    tick_ledger_maintenance_at(server, now);
    if server.state.lock().unwrap().admission_frozen() {
        return;
    }
    // tmux has no Codex turn-readiness signal, and user-input collision is
    // outside this contract. Deadline wakeups use the normal notification
    // path; pane output is observation only, never a consumption receipt.
    tick_with_deadline_wake_at(server, now);
}

#[cfg(test)]
fn tick_with_idle(server: &Arc<Server>, _can_receive: &dyn Fn(&str) -> bool) {
    tick_at(server, now_ms());
}

#[cfg(test)]
fn tick_with_idle_at(server: &Arc<Server>, now: i64, _can_receive: &dyn Fn(&str) -> bool) {
    tick_at(server, now);
}

fn tick_with_deadline_wake_at(server: &Arc<Server>, now: i64) {
    if server.state.lock().unwrap().admission_frozen() {
        return;
    }
    let checks: Vec<(String, String, Option<SelectedTransport>)> = {
        let state = server.state.lock().unwrap();
        state
            .notification_subscriptions
            .values()
            .filter(|s| s.status == "armed" && s.expires_ms > now)
            .map(|s| {
                (
                    s.id.clone(),
                    s.worker_id.clone(),
                    state
                        .workers
                        .get(&s.worker_id)
                        .and_then(super::selected_transport_for_worker),
                )
            })
            .collect()
    };
    let mut lost_sub_ids = Vec::new();
    for (id, worker_id, transport) in checks {
        let Some(transport) = transport else {
            lost_sub_ids.push(id);
            continue;
        };
        // Tmux endpoint identity is persisted at registration. A notification
        // attempt performs its own bounded pane check; timers do not infer
        // agent state from terminal output.
        if !super::subscription_matches_transport_by_worker(server, &id, &worker_id, &transport) {
            lost_sub_ids.push(id);
        }
    }

    let mut lifecycle_events = Vec::new();
    {
        let state = server.state.lock().unwrap();
        for subscription in state.notification_subscriptions.values() {
            if subscription.status == "armed" {
                if subscription.expires_ms <= now {
                    lifecycle_events.push(Event::NotificationStatus {
                        subscription_id: subscription.id.clone(),
                        status: "expired".into(),
                        updated_ms: now,
                    });
                } else if lost_sub_ids.contains(&subscription.id) {
                    lifecycle_events.push(Event::NotificationStatus {
                        subscription_id: subscription.id.clone(),
                        status: "transport-lost".into(),
                        updated_ms: now,
                    });
                }
            }
        }
        let live_master_probe = super::live_master_id(server, &state);
        let live_master = live_master_probe.clone().ok().flatten();
        let mut subscriptions: Vec<_> = state.notification_subscriptions.values().collect();
        subscriptions
            .sort_by_key(|subscription| (subscription.created_ms, subscription.id.clone()));
        for subscription in subscriptions {
            if subscription.status != "armed"
                || !is_goal_deadline(subscription)
                || subscription.expires_ms <= now
                || lost_sub_ids.contains(&subscription.id)
            {
                continue;
            }
            let Ok(live_master) = &live_master_probe else {
                crate::server::presence::append_log(
                    &server.log_path(),
                    &format!(
                        "TIMER_LIVE_MASTER_UNKNOWN: {}",
                        live_master_probe.as_ref().unwrap_err()
                    ),
                );
                continue;
            };
            if live_master.as_deref() != Some(subscription.worker_id.as_str()) {
                lifecycle_events.push(Event::NotificationSuppressed {
                    subscription_id: subscription.id.clone(),
                    status: "suppressed".into(),
                    reason: "goal-deadline-requires-live-master".into(),
                    updated_ms: now,
                });
            } else if subscription.fired_count > 0 {
                lifecycle_events.push(Event::NotificationSuppressed {
                    subscription_id: subscription.id.clone(),
                    status: "consumed".into(),
                    reason: "goal-deadline-one-shot-already-fired".into(),
                    updated_ms: now,
                });
            }
        }
        for task in state.tasks.values() {
            let Some(wait) = task.wait.as_ref() else {
                continue;
            };
            if task.status != "waiting" || wait.deadline_ms > now {
                continue;
            }
            let mut expired = task.clone();
            expired.status = "blocked".into();
            expired.next_step = Some(format!(
                "WAIT_TIMEOUT waiting_for={} responsible_actor={} reason={} escalation={}",
                wait.waiting_for, wait.responsible_actor, wait.reason, wait.escalation
            ));
            expired.wait = None;
            expired.updated_ms = now;
            lifecycle_events.push(Event::TaskUpdated { task: expired });
            lifecycle_events.push(Event::MasterWakeSignal {
                signal: crate::server::state::MasterWakeSignal::TaskBlocked {
                    task_id: task.id.clone(),
                },
                at_ms: now,
            });

            if let Some(master_id) = live_master.as_ref() {
                let message_id = super::gen_msg_id();
                lifecycle_events.push(Event::Sent {
                    msg: Message {
                        id: message_id.clone(),
                        from: "collab-server".into(),
                        to: master_id.clone(),
                        mtype: "notify".into(),
                        subject: Some(format!("wait-timeout:{}", task.id)),
                        body: format!(
                            "Task {} is blocked after WAIT_TIMEOUT: waiting_for={} responsible_actor={} reason={} escalation={} resume_on={}. Inspect the blocker and task graph, then resolve or reassign it.",
                            task.id,
                            wait.waiting_for,
                            wait.responsible_actor,
                            wait.reason,
                            wait.escalation,
                            wait.resume_on.join(",")
                        ),
                        in_reply_to: None,
                        created_ms: now,
                        state: "pending".into(),
                        wake_attempt_count: 0,
                        last_wake_attempt_ms: 0,
                retry_attempted: false,
                    },
                });
                if let Some(subscription) =
                    state.matching_subscription(master_id, "direct-message", None, now)
                {
                    lifecycle_events.push(Event::WakeBound {
                        message_id,
                        subscription_id: subscription.id.clone(),
                    });
                }
            }
        }
    }
    if !lifecycle_events.is_empty() {
        server.commit(&lifecycle_events);
    }

    let mut due_events = Vec::new();
    {
        let state = server.state.lock().unwrap();
        let mut idle_gate_records = HashMap::new();
        let mut goal_deadline_keys = state
            .wake_bindings
            .iter()
            .filter_map(|(message_id, subscription_id)| {
                let message = state.msgs.get(message_id)?;
                if message.state != "pending" {
                    return None;
                }
                let subscription = state.notification_subscriptions.get(subscription_id)?;
                let Some(key) = goal_deadline_key(subscription) else {
                    return None;
                };
                (key.2 <= now).then_some(key)
            })
            .collect::<HashSet<_>>();
        let live_master = match super::live_master_id(server, &state) {
            Ok(live_master) => live_master,
            Err(error) => {
                crate::server::presence::append_log(
                    &server.log_path(),
                    &format!("TIMER_LIVE_MASTER_UNKNOWN: {error}"),
                );
                None
            }
        };
        let mut subscriptions: Vec<_> = state.notification_subscriptions.values().collect();
        subscriptions
            .sort_by_key(|subscription| (subscription.created_ms, subscription.id.clone()));
        for subscription in subscriptions {
            let next_trigger = next_subscription_trigger(subscription);
            let master_idle_gate_open = if subscription.event == "master-idle" {
                match state.keepalives.get(&subscription.worker_id) {
                    None => false,
                    Some(record) => {
                        let record = idle_gate_records
                            .entry(subscription.worker_id.clone())
                            .or_insert_with(|| record.clone());
                        let pending_keepalive_notice = state.msgs.values().any(|message| {
                            message.to == subscription.worker_id
                                && message.state == "pending"
                                && message
                                    .subject
                                    .as_deref()
                                    .is_some_and(|subject| subject.starts_with("master-idle"))
                        });
                        record.idle_episode_notices < 3
                            && !record.idle_episode_stopped
                            && record.idle_since_ms > subscription.created_ms
                            && record.last_notice_ms != now
                            && !pending_keepalive_notice
                    }
                }
            } else {
                true
            };
            let master_idle_ready = if subscription.event == "master-idle" {
                live_master.as_deref() == Some(subscription.worker_id.as_str())
                    && state
                        .keepalives
                        .get(&subscription.worker_id)
                        .is_some_and(|record| record.observed == "idle" && record.idle_since_ms > 0)
                    && !state.tasks.values().any(|task| {
                        task.owner == subscription.worker_id
                            && super::keepalive::actionable(&task.status)
                    })
            } else {
                true
            };
            if subscription.status != "armed"
                || !server.config.timers.enabled
                || !matches!(subscription.event.as_str(), "deadline" | "master-idle")
                || (is_goal_deadline(subscription)
                    && live_master.as_deref() != Some(subscription.worker_id.as_str()))
                || !master_idle_ready
                || !master_idle_gate_open
                || next_trigger.is_none_or(|trigger| trigger > now)
                || state.wake_bindings.iter().any(|(message_id, bound)| {
                    bound == &subscription.id
                        && state
                            .msgs
                            .get(message_id)
                            .is_some_and(|message| message.state == "pending")
                })
            {
                continue;
            }
            if is_goal_deadline(subscription) {
                let Some(key) = goal_deadline_key(subscription) else {
                    continue;
                };
                if !goal_deadline_keys.insert(key) {
                    continue;
                }
                let revision = next_trigger
                    .and_then(|trigger| u64::try_from(trigger).ok())
                    .unwrap_or(0);
                due_events.push(Event::MasterWakeSignal {
                    signal: crate::server::state::MasterWakeSignal::GoalDue { revision },
                    at_ms: now,
                });
            }
            let message_id = super::gen_msg_id();
            if subscription.event == "master-idle" {
                let record = idle_gate_records
                    .get_mut(&subscription.worker_id)
                    .expect("master-idle gate record");
                record.idle_episode_notices = record.idle_episode_notices.saturating_add(1);
                record.last_notice_ms = now;
                due_events.push(Event::KeepaliveUpdated {
                    worker_id: subscription.worker_id.clone(),
                    record: record.clone(),
                });
            }
            due_events.extend([
                Event::Sent {
                    msg: Message {
                        id: message_id.clone(),
                        from: "collab-server".into(),
                        to: subscription.worker_id.clone(),
                        mtype: "notification".into(),
                        subject: subscription.subject.as_ref().map(|subject| {
                            if subscription.event == "master-idle" {
                                format!("master-idle:{subject}")
                            } else {
                                format!("deadline:{subject}")
                            }
                        }),
                        body: if subscription.event == "master-idle" {
                            let mut pending_merges: Vec<_> =
                                state.pending_merges.keys().cloned().collect();
                            pending_merges.sort();
                            format!("MASTER_IDLE_WAKE subject={} pending_merges={} scheduling continues; inspect actionable tasks, pending merges, and authorized open bugs. Merge accepted candidates by recording collab task integrated after moving the commit onto refs/heads/main. Cancel this subscription only iff no actionable task, dependency, resolvable blocker, or authorized open bug remains: collab notify unsubscribe {}", subscription.subject.as_deref().unwrap_or_default(), pending_merges.join(","), subscription.id)
                        } else {
                            format!("DEADLINE_REACHED subject={}{}", subscription.subject.as_deref().unwrap_or_default(), if subscription.fired_count + 1 >= if subscription.interval_ms.is_some() { subscription.repeat_count } else { subscription.trigger_times_ms.len().max(1) as u32 } { "; LAST_REMINDER=true; renew explicitly: collab notify subscribe --event deadline --subject <subject> --at-ms <future-epoch-ms> --ttl-seconds <bounded>" } else { "" })
                        },
                        in_reply_to: None,
                        created_ms: now,
                        state: "pending".into(),
                        wake_attempt_count: 0,
                        last_wake_attempt_ms: 0,
                retry_attempted: false,
                    },
                },
                Event::WakeBound {
                    message_id: message_id.clone(),
                    subscription_id: subscription.id.clone(),
                },
            ]);
        }
    }
    if !due_events.is_empty() {
        server.commit(&due_events);
    }

    let candidates: Vec<(String, String)> = {
        let state = server.state.lock().unwrap();
        state
            .wake_bindings
            .iter()
            .filter_map(|(message_id, subscription_id)| {
                let message = state.msgs.get(message_id)?;
                let subscription = state.notification_subscriptions.get(subscription_id)?;
                (message.state == "pending"
                    && message.wake_attempt_count < MAX_WAKE_ATTEMPTS
                    && !super::mailbox::is_explicit_delivery_mode(&state, message)
                    && super::mailbox::automatic_retry_eligible(
                        message,
                        server.config.notifications.delay_ms(&subscription.event),
                        now,
                    )
                    && subscription.status == "armed"
                    && subscription.expires_ms > now)
                    .then(|| (message_id.clone(), subscription_id.clone()))
            })
            .collect()
    };
    for (message_id, subscription_id) in candidates {
        super::attempt_notification_with_at(server, &message_id, &subscription_id, now);
    }
}

fn tick_ledger_maintenance_at(server: &Arc<Server>, now: i64) {
    if !server.config.timers.enabled {
        return;
    }
    let bindings = {
        let state = server.state.lock().unwrap();
        state
            .global
            .current_thread_routes
            .values()
            .cloned()
            .collect::<Vec<_>>()
    };
    let mut classified = 0u32;
    let mut transitioned = 0u32;
    let mut unchanged = 0u32;
    let mut blocked = 0u32;
    let mut events = Vec::new();
    for binding in bindings {
        let Some(worker) = ({
            let state = server.state.lock().unwrap();
            // The ledger is resident control state. A host-wide route whose
            // project scope this daemon never registered is not reducible here:
            // the global reducer rejects the record, so classifying it would
            // append an event that replay can never reduce again.
            let owned = state
                .global
                .lookup_registration(&binding.project_scope, &binding.app_scope_id)
                .is_some()
                && state.global.lookup_binding(&binding.binding_id).is_some();
            owned
                .then(|| state.workers.get(binding.agent_id.as_str()).cloned())
                .flatten()
        }) else {
            blocked = blocked.saturating_add(1);
            continue;
        };
        let (state, reason) = match super::worker_presence(server, &worker) {
            super::presence::IdentityPresence::Present => (RuntimeBindingLedgerState::Live, None),
            super::presence::IdentityPresence::Cold => (RuntimeBindingLedgerState::Cold, None),
            super::presence::IdentityPresence::Missing => (
                RuntimeBindingLedgerState::Missing,
                Some("registered transport probe returned missing; durable mailbox remains authoritative"),
            ),
            super::presence::IdentityPresence::Unknown => (
                RuntimeBindingLedgerState::RepairRequired,
                Some("transport probe returned unknown; preserving route and identity records"),
            ),
        };
        let operation_id = crate::identity::OperationId::new(format!("ledger-{now}-{}", binding.binding_id)).ok();
        let receipt_id = crate::identity::OperationId::new(format!("ledger-receipt-{now}-{}", binding.binding_id)).ok();
        let (operation_id, receipt_id) = match (operation_id, receipt_id) {
            (Some(operation_id), Some(receipt_id)) => (operation_id, receipt_id),
            _ => {
                blocked = blocked.saturating_add(1);
                continue;
            }
        };
        let existing = server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_runtime_binding_ledger(
                &binding.project_scope,
                &binding.app_scope_id,
                &binding.binding_id,
            )
            .cloned();
        if existing
            .map(|record| {
                record.endpoint_generation == binding.endpoint_generation
                    && record.state == state
                    && record.probe_state == Some(state)
            })
            .unwrap_or(false)
        {
            unchanged = unchanged.saturating_add(1);
            continue;
        }
        classified = classified.saturating_add(1);
        transitioned = transitioned.saturating_add(1);
        events.push(Event::GlobalRuntimeBindingLedgerClassified {
            record: RuntimeBindingLedgerRecord {
                project_scope: binding.project_scope.clone(),
                app_scope_id: binding.app_scope_id.clone(),
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                binding_id: binding.binding_id.clone(),
                endpoint_generation: binding.endpoint_generation,
                state,
                probe_state: Some(state),
                reason: reason.map(str::to_owned),
                classified_ms: now,
                operation_id,
                receipt_id: receipt_id.to_string(),
            },
        });
    }
    if classified > 0 || blocked > 0 {
        let scan_id = format!("ledger-scan-{now}");
        let scan_operation_id = crate::identity::OperationId::new(format!("ledger-scan-op-{now}")).ok();
        let scan_receipt_id = crate::identity::OperationId::new(format!("ledger-scan-receipt-{now}")).ok();
        if scan_operation_id.is_some() && scan_receipt_id.is_some() {
            // Scan receipts are evidence, not delivery truth. The mailbox is not
            // changed by classification, so the durable message count is unchanged.
            events.push(Event::GlobalLedgerScanReceiptRecorded {
                receipt: LedgerScanReceipt {
                    scan_id,
                    scanned_ms: now,
                    classified,
                    transitioned,
                    unchanged,
                    blocked,
                    mailbox_messages_unchanged: true,
                },
            });
        }
    }
    if !events.is_empty() {
        server.commit(&events);
    }
}

fn next_subscription_trigger(
    subscription: &crate::server::state::NotificationSubscription,
) -> Option<i64> {
    subscription
        .interval_ms
        .map(|interval| {
            subscription
                .trigger_ms
                .unwrap_or(subscription.created_ms.saturating_add(interval))
        })
        .or_else(|| {
            subscription
                .trigger_times_ms
                .get(subscription.fired_count as usize)
                .copied()
        })
        .or(subscription.trigger_ms)
}

#[cfg(test)]
#[path = "timers_tests.rs"]
mod tests;
