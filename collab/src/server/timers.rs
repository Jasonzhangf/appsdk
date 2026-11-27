use crate::server::state::{
    goal_deadline_key, is_goal_deadline, now_ms, Event, GoalIdleWakeState, GoalStatusNotice, Message,
    NotificationSubscription, MAX_WAKE_ATTEMPTS,
};
use crate::server::Server;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

const STALE_IDENTITY_MAINTENANCE_INTERVAL_MS: i64 = 10 * 60 * 1000;
static STALE_IDENTITY_MAINTENANCE_WATERMARKS: OnceLock<Mutex<HashMap<std::path::PathBuf, i64>>> =
    OnceLock::new();

/// Test-only pause between deriving a scheduler due batch and committing it.
/// Regression tests use it to force the goal-reconcile interleaving that the
/// real guard now prevents. The pair names the one subscription whose due
/// batch engages the pause, so parallel timer tests cannot hit it.
#[cfg(test)]
pub(super) static TIMER_DUE_COMMIT_PAUSE: std::sync::Mutex<
    Option<(String, Box<dyn Fn() + Send + Sync>)>,
> = std::sync::Mutex::new(None);

#[cfg(test)]
fn timer_due_commit_pause_hook(due_events: &[Event]) {
    let hook = TIMER_DUE_COMMIT_PAUSE.lock().unwrap();
    let Some((subscribe_id, hook)) = hook.as_ref() else {
        return;
    };
    let batch_targets_this_subscription = due_events.iter().any(|event| {
        matches!(
            event,
            Event::WakeBound { subscription_id, .. } if subscription_id == subscribe_id
        )
    });
    if batch_targets_this_subscription {
        hook();
    }
}

/// Server-side scheduler for finite subscriptions and bounded waits. Ordinary
/// work never gets a wake from elapsed time alone; the agent monitor may still
/// continue an admitted ticket queue when it observes a trusted online-idle
/// slot.
pub fn tick(server: &Arc<Server>) {
    tick_at(server, now_ms());
}

pub(super) fn tick_managed(manager: &super::ProjectRuntimeManager, server: &Arc<Server>) {
    let now = now_ms();
    super::keepalive::tick_at(server, now);
    super::name_monitor::tick_managed(manager, server, now);
    tick_after_tmux(server, now);
}

fn tick_at(server: &Arc<Server>, now: i64) {
    super::keepalive::tick_with_rex_recovery(server, now);
    super::name_monitor::tick(server, now);
    tick_after_tmux(server, now);
}

fn tick_after_tmux(server: &Arc<Server>, now: i64) {
    super::rex_monitor::reconcile(server, now);
    #[cfg(test)]
    super::name_stewardship::tick(server, now);
    super::purge_expired_storage(server, now);
    tick_default_lease_maintenance_at(server, now);
    if server.state.lock().unwrap().admission_frozen() {
        return;
    }
    // tmux has no Codex turn-readiness signal, and user-input collision is
    // outside this contract. Deadline wakeups use the normal notification
    // path; pane output is observation only, never a consumption receipt.
    tick_with_deadline_wake_at(server, now);
    if stale_identity_maintenance_due(&server.root, now) {
        super::peer_lifecycle::maintain_definitive_missing(server.as_ref(), now);
    }
}

fn stale_identity_maintenance_due(root: &Path, now: i64) -> bool {
    let watermarks =
        STALE_IDENTITY_MAINTENANCE_WATERMARKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut watermarks = watermarks.lock().unwrap();
    let due = watermarks
        .get(root)
        .is_none_or(|last| now >= last.saturating_add(STALE_IDENTITY_MAINTENANCE_INTERVAL_MS));
    if due {
        watermarks.insert(root.to_path_buf(), now);
    }
    due
}

/// Refresh daemon-owned default direct-message leases from ordinary
/// maintenance. This owner is independent of `timers.enabled`, which controls
/// finite schedule emission rather than base reception. The cheap gate scans
/// only registered workers and their own lease row; the message-wide reconcile
/// owner runs only when a lease is missing, retired, bound to a stale
/// transport, or due for half-life renewal.
fn tick_default_lease_maintenance_at(server: &Arc<Server>, now: i64) {
    let needs_reconcile = {
        let state = server.state.lock().unwrap();
        let refresh_after_ms = super::DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000 / 2;
        state.workers.values().any(|worker| {
            let Some(transport) = super::selected_transport_for_worker(worker) else {
                return false;
            };
            let Some(lease) = state
                .notification_subscriptions
                .get(&super::default_direct_message_id(&worker.id))
            else {
                return true;
            };
            lease.status != "armed"
                || !super::subscription_matches_transport(lease, &transport)
                || lease.expires_ms <= now
                || lease.expires_ms - now < refresh_after_ms
        })
    };
    if needs_reconcile {
        super::restore_registered_peer_default_leases_at(server, now);
    }
}

#[cfg(test)]
fn tick_with_idle(server: &Arc<Server>, _can_receive: &dyn Fn(&str) -> bool) {
    tick_at(server, now_ms());
}

#[cfg(test)]
pub(super) fn tick_with_idle_at(
    server: &Arc<Server>,
    now: i64,
    _can_receive: &dyn Fn(&str) -> bool,
) {
    tick_at(server, now);
}

fn tick_with_deadline_wake_at(server: &Arc<Server>, now: i64) {
    if server.state.lock().unwrap().admission_frozen() {
        return;
    }
    // The standing project-patrol resource has its own owner. Run it before the
    // legacy finite-timer locks so the generic master-idle lifecycle never acts
    // on it, and so the native activity read happens without holding a lock.
    tick_standing_project_patrol_at(server, now);
    let mut lifecycle_events = Vec::new();
    {
        let mut state = server.state.lock().unwrap();
        // Scheduling and mailbox admission do not depend on whether the
        // current carrier can wake. The delivery attempt owns availability.
        for subscription in state.notification_subscriptions.values() {
            if subscription.status == "armed" && subscription.expires_ms <= now {
                lifecycle_events.push(Event::NotificationStatus {
                    subscription_id: subscription.id.clone(),
                    status: "expired".into(),
                    updated_ms: now,
                });
            }
        }
        // Project wait escalation uses master authority; owned goal deadlines
        // are independent of that grant and its transport reachability.
        let master_holder = super::current_master_holder(server, &state);
        let mut subscriptions: Vec<_> = state.notification_subscriptions.values().collect();
        subscriptions
            .sort_by_key(|subscription| (subscription.created_ms, subscription.id.clone()));
        for subscription in subscriptions {
            if subscription.status != "armed"
                || !is_goal_deadline(subscription)
                || subscription.expires_ms <= now
            {
                continue;
            }
            // An explicit periodic goal deadline legitimately has
            // `fired_count > 0` between occurrences, so only suppress it once
            // its repeat budget is exhausted. Legacy and non-periodic goal
            // occurrences keep the one-shot compatibility suppression.
            if subscription.periodic_declared {
                if subscription.fired_count >= subscription.repeat_count {
                    lifecycle_events.push(Event::NotificationSuppressed {
                        subscription_id: subscription.id.clone(),
                        status: "consumed".into(),
                        reason: "goal-deadline-periodic-exhausted".into(),
                        updated_ms: now,
                    });
                }
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

            if let Ok(Some(master_id)) = master_holder.as_ref() {
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
        if !lifecycle_events.is_empty() {
            server.commit_locked_reporting(&mut state, &lifecycle_events);
        }
    }

    let mut due_events = Vec::new();
    {
        let mut state = server.state.lock().unwrap();
        let mut idle_gate_records = HashMap::new();
        let mut goal_idle_updates = HashMap::new();
        let unread_periodic_goals: HashSet<_> = state.notification_occurrences.iter()
            .filter(|(id, occurrence)| {
                occurrence.consumed_ms.is_none() && state.msgs.get(*id).is_some_and(|message| {
                    matches!(message.state.as_str(), "pending" | "delivered")
                })
            })
            .map(|(_, occurrence)| occurrence.subscription_id.as_str())
            .collect();
        let goal_idle_scope = state
            .notification_subscriptions
            .values()
            .any(|sub| owned_periodic_goal(sub) && sub.status == "armed" && goal_can_resume(&state, sub))
            .then(|| {
                let route = super::server_route_scope(server, &state).ok().flatten()?;
                let control = super::binding_store::BindingControlStore::new(
                    server.host_paths.state_root(),
                )
                .load().ok()?;
                Some((route, control))
            })
            .flatten();
        let mut goal_deadline_keys = state
            .wake_bindings
            .iter()
            .filter_map(|(message_id, subscription_id)| {
                let message = state.msgs.get(message_id)?;
                let subscription_id = state.notification_occurrences.get(message_id)
                    .filter(|occurrence| occurrence.consumed_ms.is_none())
                    .map_or(subscription_id, |occurrence| &occurrence.subscription_id);
                let subscription = state.notification_subscriptions.get(subscription_id)?;
                let key = goal_deadline_key(subscription)?;
                match message.state.as_str() {
                    "pending" if key.2 <= now || owned_periodic_goal(subscription) => {
                        Some((key, super::task_schedule_binding(subscription)))
                    }
                    "delivered" if owned_periodic_goal(subscription) => {
                        Some((key, super::task_schedule_binding(subscription)))
                    }
                    _ => None,
                }
            })
            .collect::<HashSet<_>>();
        let master_holder = match super::current_master_holder(server, &state) {
            Ok(master_holder) => master_holder,
            Err(error) => {
                crate::server::presence::append_log(
                    &server.log_path(),
                    &format!("TIMER_MASTER_SCOPE_UNKNOWN: {error}"),
                );
                None
            }
        };
        // A genuine external gate suppresses finite master-idle notices only.
        // Deadlines and every other event keep their normal behavior, and the
        // suppressed occurrence is not consumed so release resumes it.
        let master_wake_hold_active = state
            .master_wake
            .wake_hold
            .as_ref()
            .is_some_and(|hold| now < hold.until_ms);
        let mut subscriptions: Vec<_> = state.notification_subscriptions.values().collect();
        subscriptions
            .sort_by_key(|subscription| (subscription.created_ms, subscription.id.clone()));
        for subscription in subscriptions {
            // The standing project-patrol resource is advanced by its own
            // dedicated pass. Keep the legacy finite master-idle gates from
            // waking it a second time or consuming its idle-episode budget.
            if is_standing_project_patrol(subscription) {
                continue;
            }
            // Quiet only task-owned execution reminders while their actor is
            // waiting. Keep the occurrence and schedule progress for release;
            // independent explicit deadlines retain their declared delivery.
            if !task_goal_actionable(&state, subscription, &subscription.worker_id) {
                continue;
            }
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
                master_holder.as_deref() == Some(subscription.worker_id.as_str())
                    && state
                        .keepalives
                        .get(&subscription.worker_id)
                        .is_some_and(|record| record.observed == "idle" && record.idle_since_ms > 0)
                    && !state.tasks.values().any(|task| {
                        state.task_actionable_for(task, Some((&subscription.worker_id, "Jenny")))
                    })
            } else {
                true
            };
            if subscription.status != "armed"
                || !server.config.timers.enabled
                || !matches!(subscription.event.as_str(), "deadline" | "master-idle")
                || !master_idle_ready
                || !master_idle_gate_open
                || (subscription.event == "master-idle" && master_wake_hold_active)
            {
                continue;
            }
            let Some(trigger) = next_trigger else { continue };
            let mut eligible_idle = false;
            let mut goal_address = None;
            if owned_periodic_goal(subscription) && goal_can_resume(&state, subscription) {
                if let Some((route, control)) = &goal_idle_scope {
                    if let Some((slot, status)) =
                        goal_idle_status(server, &state, route, control, &subscription.worker_id)
                    {
                        let current = slot.active_binding().expect("current slot");
                        let binding_id = current.binding.binding_id.to_string();
                        let gate = goal_idle_updates.entry(subscription.worker_id.clone())
                            .or_insert_with(|| state.goal_idle_wake
                                .get(&subscription.worker_id).cloned().unwrap_or_default());
                        if gate.binding_id != binding_id || gate.generation != slot.generation() {
                            clear_goal_status_notice(&state, gate, &mut due_events);
                            *gate = GoalIdleWakeState {
                                binding_id,
                                generation: slot.generation(),
                                ..GoalIdleWakeState::default()
                            };
                        }
                        match status {
                            GoalIdleStatus::Idle => {
                                clear_goal_status_notice(&state, gate, &mut due_events);
                                // An occurrence issued at its normal deadline may have
                                // waited in the mailbox for this first eligible idle.
                                if unread_periodic_goals.contains(subscription.id.as_str()) {
                                    gate.issued.insert(subscription.id.clone());
                                }
                                eligible_idle = !gate.issued.contains(&subscription.id);
                                goal_address = Some(super::name_mailbox::MailboxNameAddress {
                                    project_scope: route.project_scope_id.as_str().into(),
                                    name: slot.identity_key.name.clone(),
                                });
                            }
                            GoalIdleStatus::Unknown { reason, missing }
                                if gate.unknown_notice.as_ref().is_none_or(|notice| notice.missing != missing) =>
                            {
                                clear_goal_status_notice(&state, gate, &mut due_events);
                                if let Some(master) = master_holder.as_deref() {
                                    gate.unknown_notice = goal_status_notice(
                                        &state, route, master, slot, &reason, &missing,
                                        now, &mut due_events,
                                    );
                                }
                            }
                            GoalIdleStatus::Inactive => {
                                gate.issued.clear();
                                clear_goal_status_notice(&state, gate, &mut due_events);
                            }
                            GoalIdleStatus::Unknown { .. } | GoalIdleStatus::NoSample => {}
                        }
                    }
                }
            }
            let idle_triggered = trigger > now && eligible_idle;
            if (trigger > now && !idle_triggered)
                || unread_periodic_goals.contains(subscription.id.as_str())
                || state.wake_bindings.iter().any(|(message_id, bound)| {
                    bound == &subscription.id && state.msgs.get(message_id).is_some_and(|message| {
                        message.state == "pending"
                            || (owned_periodic_goal(subscription) && message.state == "delivered")
                    })
                })
            {
                continue;
            }
            if is_goal_deadline(subscription) {
                let Some(key) = goal_deadline_key(subscription) else {
                    continue;
                };
                if !goal_deadline_keys.insert((key, super::task_schedule_binding(subscription))) {
                    continue;
                }
                let revision = next_trigger
                    .and_then(|trigger| u64::try_from(trigger).ok())
                    .unwrap_or(0);
                if master_holder.as_deref() == Some(subscription.worker_id.as_str()) {
                    due_events.push(Event::MasterWakeSignal {
                        signal: crate::server::state::MasterWakeSignal::GoalDue { revision },
                        at_ms: now,
                    });
                }
            }
            let goal_body = match subscription
                .goal_content
                .as_ref()
                .and_then(|binding| binding.source.as_ref())
            {
                Some(super::state::GoalScheduleContent::Inline { content }) => {
                    Some(content.clone())
                }
                Some(super::state::GoalScheduleContent::BoardTask { task_id }) => {
                    let Some(details) = state.board_details.get(task_id) else {
                        crate::server::presence::append_log(
                            &server.log_path(),
                            "GOAL_TASK_CONTENT_MISSING: referenced task has no details",
                        );
                        continue;
                    };
                    let Some(spec) = details.long_horizon.as_ref() else {
                        crate::server::presence::append_log(
                            &server.log_path(),
                            "GOAL_TASK_CONTENT_MISSING: task has no long-horizon declaration",
                        );
                        continue;
                    };
                    let status = state
                        .tasks
                        .get(task_id)
                        .map(|task| task.status.as_str())
                        .unwrap_or("missing");
                    let action = "Continue only the task's current authorized phase and dependency constraints.";
                    Some(format!("Task {task_id}\n{}\n{}\nDelivery condition: {}\nTest condition: {}\nCurrent status: {status}\n{action}",
                        details.title, spec.content, details.delivery_condition, details.test_condition))
                }
                None => None,
            };
            if goal_address.is_some() {
                goal_idle_updates.get_mut(&subscription.worker_id)
                    .expect("idle status initialized the gate")
                    .issued.insert(subscription.id.clone());
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
                            format!("MASTER_IDLE_WAKE: {GOAL_PATROL_RULE} Assign ready authorized tasks from the current board. Each delivery owner integrates their own candidate. When no actionable work remains, use collab notify cancel --event master-idle with the evaluated reason and observable resume condition. Cancel a separate finished goal through appsdk goal cancel. Retain direct-message reception.")
                        } else if let Some(content) = &goal_body {
                            let trigger_label = if idle_triggered {
                                "GOAL_READY_ON_IDLE"
                            } else {
                                "DEADLINE_REACHED"
                            };
                            let instruction = if owned_periodic_goal(subscription) {
                                goal_owner_instruction(subscription)
                            } else {
                                ""
                            };
                            format!("{trigger_label}\n{instruction}{content}\nCycle {}/{}; interval_ms={:?}; expires_ms={}\n{}",
                                subscription.fired_count + 1, subscription.repeat_count, subscription.interval_ms, subscription.expires_ms,
                                super::workflow_template_rule("execution_cycle_rule"))
                        } else {
                            format!("DEADLINE_REACHED{}", if subscription.fired_count + 1 >= if subscription.interval_ms.is_some() { subscription.repeat_count } else { subscription.trigger_times_ms.len().max(1) as u32 } { if subscription.subject.as_deref().is_some_and(|subject| subject.starts_with("goal:")) { "; LAST_REMINDER=true; renew explicitly with `appsdk goal subscribe` for the owned goal only when continued scheduling is required" } else { "; LAST_REMINDER=true; renew explicitly with `collab notify subscribe --event deadline --subject <subject> --at-ms <future-epoch-ms> --ttl-seconds <bounded>` only when continued scheduling is required" } } else { "" })
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
            if is_goal_deadline(subscription) && subscription.periodic_declared {
                if idle_triggered {
                    due_events.push(Event::MailboxNameAddressed {
                        message_id: message_id.clone(),
                        address: goal_address.expect("idle wake has a current goal owner"),
                    });
                }
                due_events.push(Event::NotificationOccurrenceIssued {
                    message_id,
                    subscription_id: subscription.id.clone(),
                    trigger_ms: trigger,
                    idle_triggered,
                });
            }
        }
        due_events.extend(goal_idle_updates.into_iter().filter_map(|(worker_id, mut value)| {
            value.issued.retain(|id| state.notification_subscriptions.get(id).is_some_and(|sub| sub.status == "armed"));
            (state.goal_idle_wake.get(&worker_id) != Some(&value))
                .then_some(Event::GoalIdleWakeUpdated { worker_id, value })
        }));
        // A due batch must land in the same guard that derived it. Goal
        // reconciliation takes this same lock, so an ended or replaced
        // occurrence is either already retired when the batch is derived, or
        // supersedes the batch in the same commit that retires it. Committing
        // after the guard is released lets a late batch resurrect a goal whose
        // receipt is already terminal, and raises the master goal-due latch
        // again.
        if !due_events.is_empty() {
            #[cfg(test)]
            timer_due_commit_pause_hook(&due_events);
            server.commit_locked_reporting(&mut state, &due_events);
        }
    }

    let candidates: Vec<_> = {
        let state = server.state.lock().unwrap();
        state
            .wake_bindings
            .iter()
            .filter_map(|(message_id, subscription_id)| {
                let message = state.msgs.get(message_id)?;
                let subscription = state.notification_subscriptions.get(subscription_id)?;
                // A standing project patrol only delivers the explicit notices
                // it created for a due occurrence. An older message bound to
                // the same schedule may carry uncertain input and is never
                // replayed by the automatic timer path.
                if is_standing_project_patrol(subscription)
                    && state.delivery_modes.get(message_id).map(String::as_str)
                        != Some("explicit-notification")
                {
                    return None;
                }
                // A name handoff grants one wake to the current carrier. Old
                // acceptance, delivered state and exhausted budget are history.
                if state.handoff_wake_pending(message) {
                    return (subscription.worker_id == message.to
                        && subscription.status == "armed"
                        && subscription.expires_ms > now)
                        .then(|| (message_id.clone(), subscription_id.clone(), false));
                }
                (message.state == "pending"
                    && (message.wake_attempt_count < MAX_WAKE_ATTEMPTS
                        || super::notification_reconnect_pending(&state, message))
                    && automatic_delivery_candidate(&state, message)
                    && super::mailbox::automatic_retry_eligible(
                        message,
                        super::notification_delivery_delay_ms(
                            &state,
                            &message.id,
                            &subscription.event,
                            &server.config.notifications,
                        ),
                        now,
                    )
                    && subscription.status == "armed"
                    && subscription.expires_ms > now)
                    .then(|| {
                        let board_invitation_only =
                            state.delivery_modes.get(message_id).map(String::as_str)
                                == Some("explicit-notification");
                        (
                            message_id.clone(),
                            subscription_id.clone(),
                            board_invitation_only,
                        )
                    })
            })
            .collect()
    };
    for (message_id, subscription_id, board_invitation_only) in candidates {
        let _ = super::attempt_background_notification(
            server,
            &message_id,
            &subscription_id,
            now,
            board_invitation_only,
        );
    }
}

/// The automatic timer candidate domain.
///
/// Ordinary messages (no persisted delivery mode) keep the existing batch
/// retry contract, and queued tmux delivery keeps its deferred path.
/// Board invitations may continue after pre-I/O deferral. Other explicit
/// notifications stay with their explicit retry operator.
fn automatic_delivery_candidate(state: &super::state::State, message: &Message) -> bool {
    match state.delivery_modes.get(&message.id).map(String::as_str) {
        None => true,
        Some("queued") => {
            let transport = state
                .workers
                .get(&message.to)
                .and_then(super::selected_transport_for_worker);
            transport.is_some_and(|transport| {
                use crate::proto::TransportKind;
                matches!(
                    transport.kind,
                    TransportKind::Tmux | TransportKind::AppServer | TransportKind::Dsh
                ) && super::can_resume_notification(state, message)
            })
        }
        Some("immediate" | super::mailbox::REGISTRATION_NOTIFICATION_MODE) => {
            super::can_resume_notification(state, message)
        }
        Some("explicit-notification") => {
            super::current_board_invitation_for_message(state, message).is_some()
                && super::can_resume_notification(state, message)
        }
        // Internal daemon markers predate the explicit delivery domain and
        // keep their pre-existing automatic path unchanged.
        Some(_) => true,
    }
}

/// Standing project patrol: a dedicated master-idle resource with the
/// `project-patrol` subject and a 25-minute target interval. The legacy
/// 15-minute interval remains accepted as a compatibility fallback so an
/// already-armed record can be upgraded by the goal owner without a second
/// patrol. It is separate
/// from finite master-idle schedules: it renews its own budget and TTL on the
/// same id while the current master obligation is active, and it is never
/// gated by the finite idle-episode caps, the master's own tasks, the idle
/// clock, or an older accepted-but-unconsumed notice.
const PROJECT_PATROL_SUBJECT: &str = "project-patrol";
const PROJECT_PATROL_TARGET_INTERVAL_MS: i64 = 1_500_000;
const PROJECT_PATROL_COMPAT_INTERVAL_MS: i64 = 900_000;
const PROJECT_PATROL_TTL_MS: i64 = 604_800_000;
const PROJECT_PATROL_REPEAT_HEADROOM: u32 = 100;
const PROJECT_PATROL_TRUSTED_IDLE_MS: i64 = 1_800_000;

fn is_project_patrol_interval(interval_ms: Option<i64>) -> bool {
    matches!(
        interval_ms,
        Some(PROJECT_PATROL_TARGET_INTERVAL_MS | PROJECT_PATROL_COMPAT_INTERVAL_MS)
    )
}

#[cfg(test)]
fn seed_project_patrol_episode(
    server: &Server,
    subscription_id: &str,
    idle_since_ms: i64,
    last_push_ms: i64,
) {
    // The seeded episode models a poll this process already made, so the
    // restart guard must not discard it.
    let _ = super::observation_epoch::first_observation_since_start(
        &server.root,
        &format!("project-patrol:{subscription_id}"),
    );
    let mut subscription =
        server.state.lock().unwrap().notification_subscriptions[subscription_id].clone();
    subscription.patrol_idle_since_ms = Some(idle_since_ms);
    subscription.patrol_last_push_ms = last_push_ms;
    server.commit(&[Event::NotificationSubscribed { subscription }]);
}

fn is_standing_project_patrol(subscription: &NotificationSubscription) -> bool {
    subscription.event == "master-idle"
        && subscription.subject.as_deref() == Some(PROJECT_PATROL_SUBJECT)
        && is_project_patrol_interval(subscription.interval_ms)
}

fn same_standing_project_patrol(
    current: &NotificationSubscription,
    snapshot: &NotificationSubscription,
) -> bool {
    is_standing_project_patrol(current)
        && current.id == snapshot.id
        && current.worker_id == snapshot.worker_id
        && current.target == snapshot.target
        && current.method == snapshot.method
        && current.trigger_ms == snapshot.trigger_ms
        && current.interval_ms == snapshot.interval_ms
        && current.created_ms == snapshot.created_ms
}

const GOAL_PATROL_RULE: &str = "For every goal, verify a nonempty Action Items field and check its items against the current acceptance criteria. Flag missing, empty, or stale items for the goal owner. Check that goal changes update Action Items in the same revision.";

fn project_patrol_body() -> String {
    format!(
        "MASTER_IDLE_WAKE. Your current activity is idle. \
         Audit four items before acting: (1) unresolved P0/P1 items; (2) unfinished or ready work; \
         (3) owned worktree and resource release; (4) peer/worker idle facts and whether each idle \
         reason is reasonable. Check current workers, unclosed board ownership and unclosed tasks, current bindings, \
         per-task board patrol cancellations with `collab notify status` (project_board_patrol view; \
         use `collab board show` for tasks), and real blockers. \
         {GOAL_PATROL_RULE} \
         Wake only peer-owned eligible uncancelled tasks; never rearm cancelled tasks. \
         The daemon board dispatcher performs the actual eligible peer wake from fresh board facts, \
         so do not duplicate it with a blind manual wake. \
         Always query the actual global CEO status with `collab ceo status`. \
         If CEO_NOT_CONFIGURED, continue local master work. \
         If configured and substantive progress or blockers changed, report the substantive goal, \
         results, remaining work, blockers, and next action through `collab ceo reply`; \
         when none remain, continue local master work. \
         Direct-message reception is daemon-owned and must never be closed.",
    )
}

/// Advance the standing project-patrol schedule for one due occurrence.
///
/// The due gate is evaluated under the state lock, the fresh native activity
/// read happens outside it, and the typed worker, current master authority and
/// the exact subscription occurrence are revalidated under the commit lock.
/// Busy, unknown, waiting, offline and read-error outcomes advance the current
/// instant by one interval so a skipped occurrence never backfills or probes
/// every second. Every due Idle occurrence creates a new explicit-notification
/// message id; the old accepted-but-unconsumed notice is left untouched.
fn tick_standing_project_patrol_at(server: &Arc<Server>, now: i64) {
    if !server.config.timers.enabled {
        return;
    }
    let candidates: Vec<(NotificationSubscription, crate::server::state::WorkerRec)> = {
        let state = server.state.lock().unwrap();
        if state.admission_frozen() {
            return;
        }
        let master_holder = match super::current_master_holder(server, &state) {
            Ok(master_holder) => master_holder,
            Err(error) => {
                super::presence::append_log(
                    &server.log_path(),
                    &format!("PROJECT_PATROL_MASTER_SCOPE_UNKNOWN: {error}"),
                );
                return;
            }
        };
        let Some(master_id) = master_holder else {
            return;
        };
        let Some(worker) = state.workers.get(&master_id).cloned() else {
            return;
        };
        let Some(transport) = super::selected_transport_for_worker(&worker) else {
            return;
        };
        state
            .notification_subscriptions
            .values()
            .filter(|subscription| {
                is_standing_project_patrol(subscription)
                    && subscription.worker_id == master_id
                    && subscription.status == "armed"
                    && subscription.repeat_count > subscription.fired_count
                    && super::subscription_matches_transport(subscription, &transport)
                    && next_subscription_trigger(subscription).is_some_and(|trigger| trigger <= now)
            })
            .cloned()
            .map(|subscription| (subscription, worker.clone()))
            .collect()
    };
    for (snapshot, worker) in candidates {
        let activity = match super::keepalive::current_board_patrol_activity(server, &worker) {
            Ok(activity) => activity,
            Err(error) => {
                super::presence::append_log(
                    &server.log_path(),
                    &format!(
                        "PROJECT_PATROL_ACTIVITY_READ_FAILED: {}: {error}",
                        worker.id
                    ),
                );
                crate::server::name_state::AgentActivity::Unknown
            }
        };
        let message_id = {
            let mut state = server.state.lock().unwrap();
            if state.admission_frozen()
                || state.workers.get(&worker.id) != Some(&worker)
                || super::current_master_holder(server, &state)
                    .ok()
                    .flatten()
                    .as_deref()
                    != Some(worker.id.as_str())
            {
                continue;
            }
            let Some(current) = state.notification_subscriptions.get(&snapshot.id).cloned() else {
                continue;
            };
            if !same_standing_project_patrol(&current, &snapshot)
                || current.status != "armed"
                || current.repeat_count <= current.fired_count
                || next_subscription_trigger(&current).is_none_or(|trigger| trigger > now)
                || !super::selected_transport_for_worker(&worker).is_some_and(|transport| {
                    super::subscription_matches_transport(&current, &transport)
                })
            {
                continue;
            }
            // A genuine external gate suppresses the notice but still advances
            // the standing schedule. An expired hold no longer suppresses it.
            let hold_active = state
                .master_wake
                .wake_hold
                .as_ref()
                .is_some_and(|hold| now < hold.until_ms);
            let is_idle = activity == crate::server::name_state::AgentActivity::Idle;
            let idle_since_ms = if is_idle {
                current
                    .patrol_idle_since_ms
                    .filter(|start| *start > 0 && *start <= now)
                    .or(Some(now))
            } else {
                None
            };
            let last_push_ms = current.patrol_last_push_ms;
            let due = idle_since_ms.is_some_and(|start| {
                now > start + PROJECT_PATROL_TRUSTED_IDLE_MS
                    && (last_push_ms == 0 || now - last_push_ms >= PROJECT_PATROL_TRUSTED_IDLE_MS)
            });
            let should_notify = due && !hold_active;
            let fired_count = if should_notify {
                current.fired_count.saturating_add(1)
            } else {
                current.fired_count
            };
            let mut renewed = current.clone();
            renewed.fired_count = fired_count;
            renewed.trigger_ms = Some(
                now.saturating_add(
                    current
                        .interval_ms
                        .unwrap_or(PROJECT_PATROL_TARGET_INTERVAL_MS),
                ),
            );
            renewed.repeat_count = renewed
                .repeat_count
                .max(fired_count.saturating_add(PROJECT_PATROL_REPEAT_HEADROOM));
            renewed.expires_ms = renewed
                .expires_ms
                .max(now.saturating_add(PROJECT_PATROL_TTL_MS));
            renewed.status = "armed".into();
            renewed.updated_ms = now;
            renewed.status_reason = None;
            renewed.patrol_idle_since_ms = idle_since_ms;
            renewed.patrol_last_push_ms = if should_notify { now } else { last_push_ms };
            let mut events = vec![Event::NotificationSubscribed {
                subscription: renewed,
            }];
            let mut message_id = None;
            if should_notify {
                let id = super::gen_msg_id();
                events.push(Event::DeliveryMode {
                    msg_id: id.clone(),
                    mode: "explicit-notification".into(),
                    source_thread_id: None,
                });
                events.push(Event::Sent {
                    msg: Message {
                        id: id.clone(),
                        from: "collab-server".into(),
                        to: worker.id.clone(),
                        mtype: "notification".into(),
                        subject: Some(format!("master-idle:{PROJECT_PATROL_SUBJECT}")),
                        body: project_patrol_body(),
                        in_reply_to: None,
                        created_ms: now,
                        state: "pending".into(),
                        wake_attempt_count: 0,
                        last_wake_attempt_ms: 0,
                        retry_attempted: false,
                    },
                });
                events.push(Event::WakeBound {
                    message_id: id.clone(),
                    subscription_id: current.id.clone(),
                });
                message_id = Some(id);
            }
            let committed = if let Err(error) = server.commit_locked_checked(&mut state, &events) {
                super::presence::append_log(
                    &server.log_path(),
                    &format!("PROJECT_PATROL_COMMIT_FAILED: {error:?}"),
                );
                false
            } else {
                true
            };
            committed.then_some(message_id).flatten()
        };
        if let Some(message_id) = message_id {
            let _ = super::attempt_notification_detailed_with_rex_recovery(
                server,
                &message_id,
                &snapshot.id,
                now,
            );
        }
    }
}

fn owned_periodic_goal(subscription: &NotificationSubscription) -> bool {
    is_goal_deadline(subscription) && subscription.periodic_declared
        && subscription.goal_content.as_ref().and_then(|binding| binding.source.as_ref()).is_some()
}

fn goal_can_resume(state: &super::State, subscription: &NotificationSubscription) -> bool {
    use super::state::GoalScheduleContent;
    match subscription.goal_content.as_ref().and_then(|binding| binding.source.as_ref()) {
        Some(GoalScheduleContent::Inline { content }) => !content.trim().is_empty(),
        Some(GoalScheduleContent::BoardTask { task_id }) => {
            state.tasks.get(task_id).is_some_and(|task| {
                !matches!(task.status.as_str(), "pending" | "invited" | "closed" | "cancelled" | "merged")
            }) && state.board_details.get(task_id)
                .and_then(|details| details.long_horizon.as_ref())
                .is_some_and(|goal| !goal.content.trim().is_empty())
        }
        None => false,
    }
}

fn worker_has_resumable_goal(state: &super::State, worker: &str, now: i64) -> bool {
    state.notification_subscriptions.values().any(|subscription| {
        subscription.worker_id == worker
            && subscription.status == "armed"
            && subscription.expires_ms > now
            && subscription.fired_count < subscription.repeat_count
            && owned_periodic_goal(subscription)
            && goal_can_resume(state, subscription)
    })
}

enum GoalIdleStatus {
    Idle,
    Inactive,
    Unknown {
        reason: String,
        missing: Vec<crate::proto::WorkerStatusField>,
    },
    NoSample,
}

fn goal_idle_status<'a>(
    server: &Server,
    state: &super::State,
    route: &crate::scope::RouteScope,
    control: &'a super::binding_slot::BindingControlSet,
    worker_id: &str,
) -> Option<(&'a super::binding_slot::IdentityBindingSlot, GoalIdleStatus)> {
    use crate::proto::TransportKind;
    let worker = state.workers.get(worker_id)?;
    let transport = super::selected_transport_for_worker(worker)?;
    if !matches!(transport.kind, TransportKind::Tmux | TransportKind::Rex) {
        return None;
    }
    let slot = control.current_for_worker(&route.project_scope_id, &route.app_scope_id, worker_id)
        .ok().flatten()?;
    let current = slot.active_binding()?;
    let binding_session = current.binding.session_id.as_ref()?;
    if current.transport.as_ref() != Some(&transport)
        || transport.session_id.as_deref() != Some(binding_session.as_str())
    {
        return None;
    }
    Some((slot, current_goal_activity(server, state, worker, &transport)))
}

fn current_goal_activity(
    server: &Server,
    state: &super::State,
    worker: &super::state::WorkerRec,
    transport: &crate::proto::SelectedTransport,
) -> GoalIdleStatus {
    use super::name_state::{AgentActivity, AgentPresence};
    if transport.kind == crate::proto::TransportKind::Rex
        && !server.rex_monitor.has_current_sample(state, &worker.id)
    {
        return GoalIdleStatus::NoSample;
    }
    let (presence, activity) = super::worker_agent_facts(server, state, worker);
    let unknown = |reason: &str| GoalIdleStatus::Unknown {
        reason: reason.into(),
        missing: super::name_monitor::missing_status_fields(presence, activity),
    };
    match (presence, activity) {
        (Some(presence), _) if presence.presence == AgentPresence::Offline => GoalIdleStatus::Inactive,
        (Some(presence), Some(activity)) if presence.presence == AgentPresence::Online => {
            match activity.activity {
                AgentActivity::Idle if !activity.background => GoalIdleStatus::Idle,
                AgentActivity::Unknown => unknown(&activity.reason),
                _ => GoalIdleStatus::Inactive,
            }
        }
        (None, None) => GoalIdleStatus::NoSample,
        (Some(presence), _) => unknown(&presence.reason),
        (_, Some(activity)) => unknown(&activity.reason),
    }
}

fn goal_owner_instruction(subscription: &NotificationSubscription) -> &'static str {
    use super::state::GoalScheduleContent;
    match subscription.goal_content.as_ref().and_then(|binding| binding.source.as_ref()) {
        Some(GoalScheduleContent::BoardTask { .. }) =>
            "First check whether this goal is still correct, has changed, or is complete. For an authorized change, read the current Board revision and use `collab board describe <task-id> --expected-revision <revision> --title <title> --description <description> --delivery-condition <condition> --test-condition <condition> --long-horizon-content <goal>`. If complete, record acceptance and close the task through its lifecycle. Otherwise continue the current authorized work.\n",
        _ =>
            "First check whether this goal is still correct, has changed, or is complete. Use `appsdk goal list --json` to identify this goal and its document. If changed, update the document then run `appsdk goal update --goal-id <id> --goal <path.md> --json`. If complete, record acceptance and use `appsdk goal cancel --goal-id <id>`. Otherwise continue the current authorized work.\n",
    }
}

fn goal_status_notice(
    state: &super::State,
    route: &crate::scope::RouteScope,
    master: &str,
    slot: &super::binding_slot::IdentityBindingSlot,
    reason: &str,
    missing: &[crate::proto::WorkerStatusField],
    now: i64,
    events: &mut Vec<Event>,
) -> Option<GoalStatusNotice> {
    let holder = state.name_occupancy(route.project_scope_id.as_str(), "Jenny")?;
    if holder.worker_id != master {
        return None;
    }
    let binding = &slot.active_binding()?.binding;
    let worker = binding.agent_id.as_str();
    let session = binding.session_id.as_ref()?.as_str();
    let name = &slot.identity_key.name;
    let generation = slot.generation();
    let subject = format!(
        "goal-status-unknown:{worker}:{name}:{}:{generation}:{session}:{missing:?}",
        binding.binding_id.as_str()
    );
    if let Some(message) = state.msgs.values().find(|message| {
        message.from == "collab-server"
            && message.subject.as_deref() == Some(subject.as_str())
            && matches!(message.state.as_str(), "pending" | "delivered")
            && state.message_receivable_by(master, message)
    }) {
        return Some(GoalStatusNotice { message_id: message.id.clone(), missing: missing.to_vec() });
    }
    let message_id = super::gen_msg_id();
    events.extend([
        Event::Sent { msg: Message {
            id: message_id.clone(),
            from: "collab-server".into(),
            to: master.into(),
            mtype: "notification".into(),
            subject: Some(subject),
            body: format!("Goal owner {name}, binding {} generation {generation}, session {session}. Missing status fields: {missing:?}. Host evidence: {reason}. Goal delivery stays pending. Run `collab worker snapshot {worker} --capture`. If it still describes this binding and shows evidence for a missing field, run `collab worker judge --capture '<judgment_capture JSON>' --basis '<visible evidence>'` with the verified `--presence` or `--activity` values for those fields.", binding.binding_id.as_str()),
            in_reply_to: None,
            created_ms: now,
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        }},
        Event::MailboxNameAddressed {
            message_id: message_id.clone(),
            address: super::name_mailbox::MailboxNameAddress {
                project_scope: route.project_scope_id.as_str().into(),
                name: "Jenny".into(),
            },
        },
    ]);
    if let Some(subscription) = state.matching_subscription(master, "direct-message", None, now) {
        events.push(Event::WakeBound { message_id: message_id.clone(), subscription_id: subscription.id.clone() });
    }
    Some(GoalStatusNotice { message_id, missing: missing.to_vec() })
}

fn clear_goal_status_notice(
    state: &super::State,
    gate: &mut GoalIdleWakeState,
    events: &mut Vec<Event>,
) {
    if let Some(notice) = gate.unknown_notice.take() {
        if super::notification_provably_unsubmitted(state, &notice.message_id) {
            events.push(Event::Superseded { ids: vec![notice.message_id] });
        }
    }
}

pub(super) fn reconcile_goal_status_judgment(
    state: &super::State,
    route: &crate::scope::RouteScope,
    master: &str,
    slot: &super::binding_slot::IdentityBindingSlot,
    capture: &crate::proto::WorkerStatusCapture,
    now: i64,
    events: &mut Vec<Event>,
) {
    let Some(gate) = state.goal_idle_wake.get(&capture.worker_id) else { return };
    let Some(prior) = gate.unknown_notice.as_ref() else { return };
    if gate.binding_id != capture.binding.binding_id.as_str() || gate.generation != slot.generation() {
        return;
    }
    let mut presence = capture.presence.as_ref();
    let mut activity = capture.activity.as_ref();
    for event in events.iter() {
        match event {
            Event::AgentPresenceObserved { presence: observed } => presence = Some(observed),
            Event::AgentActivityObserved { activity: observed } => activity = Some(observed),
            _ => {}
        }
    }
    let missing = super::name_monitor::missing_status_fields(presence, activity);
    let goal_is_active = worker_has_resumable_goal(state, &capture.worker_id, now);
    if goal_is_active && missing == prior.missing {
        return;
    }
    let mut gate = gate.clone();
    clear_goal_status_notice(state, &mut gate, events);
    if goal_is_active && !missing.is_empty() {
        gate.unknown_notice = goal_status_notice(
            state, route, master, slot, "the current master verified part of the status",
            &missing, now, events,
        );
    }
    events.push(Event::GoalIdleWakeUpdated { worker_id: capture.worker_id.clone(), value: gate });
}

pub(super) fn goal_status_notice_current(server: &Server, state: &super::State, message: &Message) -> bool {
    if message.from != "collab-server"
        || !message.subject.as_deref().is_some_and(|subject| subject.starts_with("goal-status-unknown:"))
    {
        return true;
    }
    let Some((worker, gate, notice)) = state.goal_idle_wake.iter().find_map(|(worker, gate)| {
        let notice = gate.unknown_notice.as_ref()?;
        (notice.message_id == message.id).then_some((worker, gate, notice))
    }) else { return false };
    if !worker_has_resumable_goal(state, worker, now_ms()) {
        return false;
    }
    let Some(route) = super::server_route_scope(server, state).ok().flatten() else { return false };
    let Ok(control) = super::binding_store::BindingControlStore::new(server.host_paths.state_root()).load()
    else { return false };
    let Some((slot, GoalIdleStatus::Unknown { missing, .. })) =
        goal_idle_status(server, state, &route, &control, worker)
    else { return false };
    slot.active_binding().is_some_and(|current| {
        current.binding.binding_id.as_str() == gate.binding_id
            && slot.generation() == gate.generation
            && missing == notice.missing
    })
}

/// Recheck goal authority and status inside the existing delivery reservation.
pub(super) fn goal_wake_delivery_allowed(
    server: &Server,
    state: &super::State,
    message: &Message,
    subscription: &NotificationSubscription,
    transport: &crate::proto::SelectedTransport,
    now: i64,
) -> bool {
    let goal = occurrence_goal(state, message).unwrap_or(subscription);
    if !task_goal_wake_allowed(state, message, goal, now) {
        return false;
    }
    if !requires_goal_idle(state, message, subscription, transport) {
        return true;
    }
    if goal.status != "armed"
        || goal.worker_id != message.to
        || goal.expires_ms <= now
        || !goal_can_resume(state, goal)
    {
        return false;
    }
    let Some(worker) = state.workers.get(&message.to) else { return false };
    matches!(transport.kind, crate::proto::TransportKind::Tmux | crate::proto::TransportKind::Rex)
        && matches!(current_goal_activity(server, state, worker, transport), GoalIdleStatus::Idle)
}

fn task_goal_actionable(
    state: &super::State,
    subscription: &NotificationSubscription,
    worker: &str,
) -> bool {
    let Some(task_id) = super::task_schedule_binding(subscription) else {
        return true;
    };
    state.tasks.get(task_id).is_some_and(|task| {
        task.owner == worker
            && worker == subscription.worker_id
            && !matches!(task.status.as_str(), "waiting" | "blocked")
            && super::task_schedule_selection(state, task)
                .is_some_and(|name| state.task_actionable_for(task, Some((worker, name))))
    })
}

pub(super) fn queued_task_occurrence_actionable(state: &super::State, message: &Message) -> bool {
    message.state != "pending"
        || state
            .notification_delivery_accepted
            .contains_key(&message.id)
        || occurrence_goal(state, message)
            .is_none_or(|goal| task_goal_actionable(state, goal, &message.to))
}

fn occurrence_goal<'a>(
    state: &'a super::State,
    message: &Message,
) -> Option<&'a NotificationSubscription> {
    // Name handoff can replace the transport lease while retaining the goal.
    state
        .notification_occurrences
        .get(&message.id)
        .and_then(|occurrence| {
            state
                .notification_subscriptions
                .get(&occurrence.subscription_id)
        })
        .or_else(|| {
            let binding = state.wake_bindings.get(&message.id)?;
            state.notification_subscriptions.get(binding)
        })
}

pub(super) fn task_wake_goal<'a>(
    state: &'a super::State,
    message: &Message,
) -> Option<&'a NotificationSubscription> {
    occurrence_goal(state, message).filter(|goal| super::task_schedule_binding(goal).is_some())
}

pub(super) fn task_goal_wake_allowed(
    state: &super::State,
    message: &Message,
    goal: &NotificationSubscription,
    now: i64,
) -> bool {
    super::task_schedule_binding(goal).is_none()
        || (goal.status == "armed"
            && goal.expires_ms > now
            && task_goal_actionable(state, goal, &message.to))
}

pub(super) fn requires_goal_idle(
    state: &super::State,
    message: &Message,
    lease: &NotificationSubscription,
    transport: &crate::proto::SelectedTransport,
) -> bool {
    use crate::proto::TransportKind;
    state.notification_occurrences.get(&message.id)
        .is_some_and(|occurrence| occurrence.idle_triggered)
        || (matches!(transport.kind, TransportKind::Tmux | TransportKind::Rex)
            && owned_periodic_goal(occurrence_goal(state, message).unwrap_or(lease)))
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

#[cfg(test)]
#[path = "stale_identity_timer_tests.rs"]
mod stale_identity_timer_tests;
