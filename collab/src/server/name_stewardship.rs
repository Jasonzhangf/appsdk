//! Timer-owned task scheduling reflow.
use super::name_state::*;
use super::*;

pub(super) struct PreparedAutomaticScheduling {
    pub(super) events: Vec<Event>,
    pub(super) notifications: Vec<(String, Option<String>)>,
}

pub(super) enum NameScheduleTransition {
    SourceReleased { project_scope: String, name: String },
    ExplicitRetirement { project_scope: String, name: String },
    ValidReturn { project_scope: String, name: String },
}

fn matching_tasks<'a>(state: &'a State, name: &'a str) -> impl Iterator<Item = &'a TaskRec> {
    state.tasks.values().filter(move |task| {
        !matches!(task.status.as_str(), "closed" | "cancelled")
            && state
                .task_assignment(&task.id)
                .is_some_and(|assignment| assignment.name == name)
    })
}

fn withdrawal_events(state: &State, name: &str, now: i64) -> Vec<Event> {
    let mut events = Vec::new();
    for task in matching_tasks(state, name) {
        let Some(mut details) = state.board_details.get(&task.id).cloned() else {
            continue;
        };
        let Some(invitation) = details.invitation.clone() else {
            continue;
        };
        if invitation
            .automatic_source
            .as_ref()
            .is_none_or(|source| source.name != name)
        {
            continue;
        }
        details.invitation = None;
        details.revision += 1;
        details.last_response = Some("automatic invitation withdrawn after identity return".into());
        if task.status == "invited" {
            let mut restored = task.clone();
            restored.status = invitation
                .original_status
                .clone()
                .unwrap_or_else(|| "pending".into());
            restored.updated_ms = now;
            events.push(Event::TaskUpdated { task: restored });
        }
        events.extend([
            Event::BoardDetailsChanged {
                task_id: task.id.clone(),
                details,
            },
            Event::Superseded {
                ids: vec![invitation.message_id],
            },
        ]);
    }
    events
}

fn idle_candidate(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    task: &TaskRec,
    source_name: &str,
) -> Result<Option<(String, String, String)>, Resp> {
    let master = current_master_worker_id(state, Some(route));
    let policy = state.effective_work_policy(&task.id);
    let mut candidates = Vec::new();
    for worker in state.workers.values() {
        if worker.id == task.owner
            || master.as_deref() == Some(worker.id.as_str())
            || is_managed_subagent(state, &worker.id)
            || !state.workers.contains_key(&worker.id)
        {
            continue;
        }
        let slot = match control.current_for_worker(
            &route.project_scope_id,
            &route.app_scope_id,
            &worker.id,
        ) {
            Ok(Some(slot)) => slot,
            Ok(None) => continue,
            Err(error) => return Err(Resp::err(error)),
        };
        let Some(current) = slot.active_binding() else {
            continue;
        };
        let Some(session_id) = current.binding.session_id.as_ref() else {
            continue;
        };
        if authoritative_send_binding(state, route, &worker.id).is_err() {
            continue;
        }
        let online = state
            .agent_presence(route.project_scope_id.as_str(), &worker.id)
            .is_some_and(|record| {
                record.presence == AgentPresence::Online && record.session_id == session_id.as_str()
            });
        let idle = state
            .agent_activity
            .get(route.project_scope_id.as_str())
            .and_then(|workers| workers.get(&worker.id))
            .is_some_and(|record| {
                record.activity == AgentActivity::Idle && record.session_id == session_id.as_str()
            });
        if !online || !idle {
            continue;
        }
        if board_named_execution_reserved(state, &worker.id, &slot.identity_key.name, &task.id) {
            continue;
        }
        if let Some(executor) = policy.executor_name.as_deref() {
            if executor != slot.identity_key.name {
                continue;
            }
        }
        if board_reassignment_ready(state, route, task, &slot.identity_key.name, &policy).is_err() {
            continue;
        }
        candidates.push((
            slot.identity_key.name.clone(),
            worker.id.clone(),
            "identity reassignment selected an online idle peer".into(),
        ));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    if candidates.is_empty() {
        return Ok(None);
    }
    let (name, worker, reason) = candidates.remove(0);
    debug_assert_ne!(name, source_name);
    Ok(Some((name, worker, reason)))
}

/// Build automatic invitations for due schedules from one checked State view.
/// The caller must own the existing register-gate -> State serialization edge
/// and commit these events with the same control set.
pub(super) fn scheduling_events(
    server: &Server,
    state: &State,
    control: &binding_slot::BindingControlSet,
    now: i64,
) -> Result<PreparedAutomaticScheduling, Resp> {
    let Some(route) = server_route_scope(server, state).map_err(Resp::err)? else {
        return Ok(PreparedAutomaticScheduling {
            events: Vec::new(),
            notifications: Vec::new(),
        });
    };
    let mut prepared = PreparedAutomaticScheduling {
        events: Vec::new(),
        notifications: Vec::new(),
    };
    // Consume each prepared invitation into the candidate before selecting the
    // next task. An invitation is a capacity reservation even though it does
    // not transfer ownership.
    let mut candidate = state.clone();
    let mut schedules: Vec<_> = state
        .name_reassignments
        .get(route.project_scope_id.as_str())
        .into_iter()
        .flat_map(|names| names.iter())
        .filter(|(_, schedule)| schedule.is_due(now))
        .collect();
    schedules.sort_by(|left, right| left.0.cmp(right.0));
    for (name, schedule) in schedules {
        let task_ids: Vec<String> = matching_tasks(&candidate, name)
            .map(|task| task.id.clone())
            .collect();
        for task_id in task_ids {
            let Some(task) = candidate.tasks.get(&task_id).cloned() else {
                continue;
            };
            let Some(details) = candidate.board_details.get(&task_id).cloned() else {
                continue;
            };
            if details.invitation.is_some() {
                continue;
            }
            let assignment_revision = candidate
                .task_assignment(&task_id)
                .map_or(0, |assignment| assignment.assignment_revision);
            let Some((selected_name, _, reason)) =
                idle_candidate(&candidate, control, &route, &task, name)?
            else {
                continue;
            };
            let source = crate::board::BoardInvitationSource {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
                assignment_revision,
                schedule_round_ms: schedule.round_ms(),
            };
            let invitation = prepare_board_invitation_locked(
                server,
                &candidate,
                control,
                &route,
                None,
                &task_id,
                &selected_name,
                details.revision,
                Some((selected_name.clone(), reason)),
                details.long_horizon.clone(),
                Some(source),
                now,
            )
            .map_err(|error| {
                Resp::err(format!(
                    "AUTO_REASSIGNMENT_PREPARE_FAILED: {}",
                    error.error.unwrap_or_default()
                ))
            })?;
            prepared.notifications.push((
                invitation.message_id.clone(),
                invitation
                    .subscription
                    .as_ref()
                    .map(|subscription| subscription.id.clone()),
            ));
            for event in &invitation.events {
                candidate.apply_checked(event).map_err(|error| {
                    Resp::err(format!("AUTO_REASSIGNMENT_CANDIDATE_FAILED: {error}"))
                })?;
            }
            prepared.events.extend(invitation.events);
        }
    }
    Ok(prepared)
}

/// Build the task-owned State events for one identity transition already
/// confirmed by the identity owner. This function never reads the map, takes a
/// lock or commits; the same checked edge supplies the control set.
pub(super) fn identity_transition_events(
    server: &Server,
    state: &State,
    control: &binding_slot::BindingControlSet,
    transition: NameScheduleTransition,
    now: i64,
) -> Result<PreparedAutomaticScheduling, Resp> {
    let (project_scope, name, transition_kind) = match transition {
        NameScheduleTransition::SourceReleased {
            project_scope,
            name,
        } => (project_scope, name, 0),
        NameScheduleTransition::ExplicitRetirement {
            project_scope,
            name,
        } => (project_scope, name, 1),
        NameScheduleTransition::ValidReturn {
            project_scope,
            name,
        } => (project_scope, name, 2),
    };
    let mut events = Vec::new();
    match transition_kind {
        0 => {
            let route = server_route_scope(server, state)
                .map_err(Resp::err)?
                .ok_or_else(|| Resp::err("NAME_SOURCE_PROJECT_SCOPE_MISSING"))?;
            if control
                .current_for_name(&route.project_scope_id, &route.app_scope_id, &name)
                .map_err(Resp::err)?
                .is_some()
            {
                return Ok(PreparedAutomaticScheduling {
                    events,
                    notifications: Vec::new(),
                });
            }
            let task_ids: std::collections::BTreeSet<_> = matching_tasks(state, &name)
                .map(|task| task.id.as_str())
                .collect();
            let mut superseded = Vec::new();
            for subscription in state.notification_subscriptions.values() {
                let task_linked = matches!(
                    subscription.goal_content.as_ref().and_then(|binding| binding.source.as_ref()),
                    Some(state::GoalScheduleContent::BoardTask { task_id })
                        if task_ids.contains(task_id.as_str())
                );
                if !task_linked
                    || !matches!(
                        subscription.status.as_str(),
                        "armed" | "notifying" | "transport-lost"
                    )
                {
                    continue;
                }
                superseded.extend(pending_messages_for_subscription(state, &subscription.id));
                events.push(Event::NotificationSuppressed {
                    subscription_id: subscription.id.clone(),
                    status: "waiting-owner".into(),
                    reason: "source name awaits its current holder".into(),
                    updated_ms: now,
                });
            }
            if !superseded.is_empty() {
                superseded.sort();
                superseded.dedup();
                events.push(Event::Superseded { ids: superseded });
            }
            if matching_tasks(state, &name).next().is_none() {
                if state.name_reassignment(&project_scope, &name).is_some() {
                    events.push(Event::NameReassignmentCleared {
                        project_scope,
                        name,
                    });
                }
            } else if state.name_reassignment(&project_scope, &name).is_none() {
                events.push(Event::NameReassignmentScheduled {
                    project_scope: project_scope.clone(),
                    name: name.clone(),
                    schedule: start_return_wait(None, &reassignment_relation_ref(&name), now),
                });
            }
            return Ok(PreparedAutomaticScheduling {
                events,
                notifications: Vec::new(),
            });
        }
        1 => {
            events.extend(withdrawal_events(state, &name, now));
            events.push(Event::NameReassignmentScheduled {
                project_scope: project_scope.clone(),
                name: name.clone(),
                schedule: eligible_after_retirement(&reassignment_relation_ref(&name), now),
            });
        }
        2 => {
            if state.name_reassignment(&project_scope, &name).is_some() {
                events.push(Event::NameReassignmentCleared {
                    project_scope: project_scope.clone(),
                    name: name.clone(),
                });
            }
            events.extend(withdrawal_events(state, &name, now));
            return Ok(PreparedAutomaticScheduling {
                events,
                notifications: Vec::new(),
            });
        }
        _ => unreachable!(),
    }

    let mut candidate = state.clone();
    for event in &events {
        candidate
            .apply_checked(event)
            .map_err(|error| Resp::err(format!("AUTO_REASSIGNMENT_CANDIDATE_FAILED: {error}")))?;
    }
    let mut automatic = scheduling_events(server, &candidate, control, now)?;
    events.append(&mut automatic.events);
    Ok(PreparedAutomaticScheduling {
        events,
        notifications: automatic.notifications,
    })
}

/// Reuse the existing timer. Notifications are committed with their progress;
/// adapter attempts run only after releasing the reducer lock.
pub(super) fn tick(server: &Server, now: i64) {
    if let Err(error) = tick_checked(server, now) {
        append_log(
            &server.log_path(),
            &format!("NAME_STEWARDSHIP_FAILED: {error}"),
        );
    }
}

fn tick_checked(server: &Server, now: i64) -> Result<(), String> {
    tick_with_notifier(server, now, &|server, id, subscription, now| {
        let _ = attempt_notification_detailed_with_at(server, id, subscription, now);
    })
}

fn tick_with_notifier(
    server: &Server,
    now: i64,
    notify: &dyn Fn(&Server, &str, &str, i64),
) -> Result<(), String> {
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load()?;
    let mut state = server.state.lock().unwrap();
    let wakes = tick_locked(server, &mut state, &control, now)?;
    drop(state);
    for (id, subscription) in wakes {
        if let Some(subscription) = subscription {
            notify(server, &id, &subscription, now);
        }
    }
    Ok(())
}

/// The production manager holds register_gate and supplies this same current
/// map through selection, linked-goal derivation and durable State publication.
pub(super) fn tick_locked(
    server: &Server,
    state: &mut State,
    control: &binding_slot::BindingControlSet,
    now: i64,
) -> Result<Vec<(String, Option<String>)>, String> {
    if state.admission_frozen() {
        return Ok(Vec::new());
    }
    let Some(route) = server_route_scope(server, &state)? else {
        return Ok(Vec::new());
    };
    let master = current_master_worker_id(&state, Some(&route));
    let subscription = master
        .as_ref()
        .and_then(|master| state.matching_subscription(master, "direct-message", None, now))
        .cloned();
    let mut events = Vec::new();
    let mut wakes = Vec::new();
    if let Some(schedules) = state
        .name_reassignments
        .get(route.project_scope_id.as_str())
    {
        for name in schedules.keys() {
            let current = control
                .current_for_name(&route.project_scope_id, &route.app_scope_id, name)?
                .and_then(|slot| slot.active_binding());
            let returned = current.is_some_and(|current| {
                state
                    .agent_presence(
                        route.project_scope_id.as_str(),
                        current.binding.agent_id.as_str(),
                    )
                    .is_some_and(|presence| {
                        presence.presence == AgentPresence::Online
                            && current
                                .binding
                                .session_id
                                .as_ref()
                                .map(|session| session.as_str())
                                == Some(presence.session_id.as_str())
                    })
            });
            if returned {
                let prepared = identity_transition_events(
                    server,
                    state,
                    control,
                    NameScheduleTransition::ValidReturn {
                        project_scope: route.project_scope_id.as_str().into(),
                        name: name.clone(),
                    },
                    now,
                )
                .map_err(|error| {
                    error
                        .error
                        .unwrap_or_else(|| "NAME_RETURN_SCHEDULE_FAILED".into())
                })?;
                events.extend(prepared.events);
            }
        }
    }
    for assignment in state.task_assignments.values() {
        let Some(task) = state.tasks.get(&assignment.task_id) else {
            continue;
        };
        if matches!(task.status.as_str(), "closed" | "cancelled") {
            continue;
        }
        let reason = assignment_unavailability(&state, &control, task, Some(&route))?;
        let active = assignment
            .reflow
            .as_ref()
            .is_some_and(|flow| flow.updated_ms == assignment.updated_ms);
        let mut next = assignment.clone();
        if let Some(reason) = reason {
            if !active {
                next.reflow = Some(TaskSchedulingReflow {
                    episode: assignment
                        .reflow
                        .as_ref()
                        .map_or(1, |flow| flow.episode + 1),
                    reason: reason.into(),
                    delivery: SchedulingReflowDelivery::Pending,
                    message_id: None,
                    updated_ms: now,
                });
                next.updated_ms = now;
            }
            let flow = next.reflow.as_mut().unwrap();
            if flow.message_id.is_none() {
                if let (Some(master), Some(subscription)) = (&master, &subscription) {
                    let id = format!(
                        "name-reflow-{}-{}-{}",
                        next.task_id, flow.episode, next.assignment_revision
                    );
                    enqueue(&mut events, &id, master, subscription, now,
                        format!("任务 {} ({}) 因原绑定不可用回流：{}。需要重新 assign；Jenny 不是执行者。", next.task_id, next.name, flow.reason));
                    wakes.push((id.clone(), Some(subscription.id.clone())));
                    flow.message_id = Some(id);
                }
            }
            if let Some(id) = &flow.message_id {
                // Sent is only enqueue. Advance delivery from the adaptor's
                // durable receipt; actual consumption remains the recv owner.
                if state.notification_delivery_accepted.contains_key(id) {
                    flow.delivery = SchedulingReflowDelivery::Delivered;
                } else if state.notification_delivery_failures.contains_key(id) {
                    flow.delivery = SchedulingReflowDelivery::Failed;
                }
            }
        } else if active {
            // Preserve the episode record. A newer assignment time closes the
            // unavailable interval without deleting its reason or message ID.
            next.updated_ms = now.max(next.updated_ms + 1);
        }
        if next != *assignment {
            // Reflow progress leaves the source assignment generation intact.
            events.push(Event::TaskAssignmentUpdated { assignment: next });
        }
    }

    for assignment in state.task_assignments.values() {
        let Some(task) = state.tasks.get(&assignment.task_id) else {
            continue;
        };
        if matches!(task.status.as_str(), "closed" | "cancelled")
            || state
                .name_reassignment(route.project_scope_id.as_str(), &assignment.name)
                .is_some()
        {
            continue;
        }
        let Some(reason) = assignment_unavailability(&state, &control, task, Some(&route))? else {
            continue;
        };
        if !matches!(reason, "name_unoccupied" | "owner_offline") {
            continue;
        }
        events.push(Event::NameReassignmentScheduled {
            project_scope: route.project_scope_id.as_str().into(),
            name: assignment.name.clone(),
            schedule: start_return_wait(None, &reassignment_relation_ref(&assignment.name), now),
        });
    }

    let mut schedule_candidate = state.clone();
    for event in &events {
        schedule_candidate
            .apply_checked(event)
            .map_err(|error| format!("NAME_STEWARDSHIP_CANDIDATE_FAILED: {error}"))?;
    }
    let automatic = scheduling_events(server, &schedule_candidate, &control, now)
        .map_err(|error| error.error.unwrap_or_else(|| "scheduling failed".into()))?;
    events.extend(automatic.events);
    wakes.extend(automatic.notifications);

    if !events.is_empty() {
        server
            .commit_locked_checked_with_control(state, &events, control)
            .map_err(|error| error.to_string())?;
    }
    Ok(wakes)
}

fn enqueue(
    events: &mut Vec<Event>,
    id: &str,
    master: &str,
    subscription: &NotificationSubscription,
    now: i64,
    body: String,
) {
    events.extend([
        Event::Sent {
            msg: Message {
                id: id.into(),
                from: "daemon".into(),
                to: master.into(),
                mtype: "notification".into(),
                subject: Some("名字工作调度".into()),
                body,
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::DeliveryMode {
            msg_id: id.into(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        },
        Event::WakeBound {
            message_id: id.into(),
            subscription_id: subscription.id.clone(),
        },
    ]);
}

#[cfg(test)]
mod tests;
