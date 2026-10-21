//! Timer-owned scheduling reflow and authenticated Jenny adjudication.
use super::name_state::*;
use super::*;

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
    let mut state = server.state.lock().unwrap();
    if state.admission_frozen() {
        return Ok(());
    }
    let Some(route) = server_route_scope(server, &state)? else {
        return Ok(());
    };
    let scope = route.project_scope_id.as_str();
    let master = current_master_worker_id(&state, Some(&route));
    let subscription = master
        .as_ref()
        .and_then(|master| state.matching_subscription(master, "direct-message", None, now))
        .cloned();
    let mut events = Vec::new();
    let mut wakes = Vec::new();
    for assignment in state.task_assignments.values() {
        let Some(task) = state.tasks.get(&assignment.task_id) else {
            continue;
        };
        if matches!(task.status.as_str(), "closed" | "cancelled") {
            continue;
        }
        let reason = assignment_unavailability(&state, task, Some(&route));
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
                    wakes.push((id.clone(), subscription.id.clone()));
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
            next.assignment_revision += 1;
            events.push(Event::TaskAssignmentUpdated { assignment: next });
        }
    }

    // Presence unknown is distinct from activity unknown. Save a typed pending
    // operation for the exact occupancy; no human message becomes control data.
    if let Some(names) = state.name_occupancies.get(scope) {
        for occupancy in names.values() {
            let unknown = state
                .agent_presence(scope, &occupancy.worker_id)
                .is_some_and(|presence| {
                    presence.session_id == occupancy.session_id
                        && presence.presence == AgentPresence::Unknown
                });
            if !unknown {
                continue;
            }
            let existing = state.name_recoveries.values().any(|recovery| {
                recovery.occupancy_ref.matches(scope, occupancy)
                    && matches!(
                        recovery.progress,
                        NameRecoveryProgress::Pending
                            | NameRecoveryProgress::AdjudicationRequested
                            | NameRecoveryProgress::Applying
                    )
            });
            if existing {
                continue;
            }
            let Some(worker) = state.workers.get(&occupancy.worker_id) else {
                continue;
            };
            let Some(binding) = current_worker_binding(
                &state,
                &route.project_scope_id,
                Some(&route.app_scope_id),
                worker,
            ) else {
                continue;
            };
            let recovery = NameRecoveryPending {
                operation_id: format!(
                    "presence-{}-{}-{}-{}",
                    binding.binding_id,
                    occupancy.name,
                    occupancy.occupancy_revision,
                    state
                        .agent_presence(scope, &occupancy.worker_id)
                        .map_or(0, |presence| presence.observed_ms)
                ),
                project_scope: scope.into(),
                target_name: occupancy.name.clone(),
                occupancy_ref: occupancy.reference(scope),
                request: NameRecoveryRequest {
                    invocation: "presence_adjudication".into(),
                    requested_name: None,
                    caller_worker_id: occupancy.worker_id.clone(),
                    caller_session_id: occupancy.session_id.clone(),
                    caller_binding: NameBindingReference {
                        app_scope_id: binding.app_scope_id.to_string(),
                        binding_id: binding.binding_id.to_string(),
                        endpoint_generation: binding.endpoint_generation,
                    },
                    replace_confirmed: false,
                    requested_ms: now,
                },
                progress: NameRecoveryProgress::Pending,
                decision: None,
                result: None,
                created_ms: now,
                updated_ms: now,
            };
            events.push(Event::NameRecoveryUpdated { recovery });
        }
    }
    // Include newly created pending records in this same durable transaction.
    let pending: Vec<_> = state
        .name_recoveries
        .values()
        .cloned()
        .chain(events.iter().filter_map(|event| {
            if let Event::NameRecoveryUpdated { recovery } = event {
                Some(recovery.clone())
            } else {
                None
            }
        }))
        .collect();
    if let (Some(master), Some(subscription)) = (&master, &subscription) {
        for mut recovery in pending {
            if recovery.project_scope != scope || recovery.progress != NameRecoveryProgress::Pending
            {
                continue;
            }
            let current = state.name_occupancy(scope, &recovery.target_name);
            if !current.is_some_and(|occupancy| recovery.occupancy_ref.matches(scope, occupancy)) {
                continue;
            }
            let snapshot_target = current
                .and_then(|occupancy| state.workers.get(&occupancy.worker_id))
                .and_then(|worker| worker.transport.as_ref());
            let id = format!("name-adjudication-{}", recovery.operation_id);
            enqueue(&mut events, &id, master, subscription, now,
                format!("请获取当前 agent snapshot 并裁决名字 {}。operation_id={}，占用引用={}，snapshot_target={}。提交 action:adjudicate_name、operation_id、verdict:online/offline/unknown、evidence。unknown 或 snapshot 不足须保留占用。", recovery.target_name, recovery.operation_id,
                    serde_json::to_string(&recovery.occupancy_ref).map_err(|error| error.to_string())?,
                    serde_json::to_string(&snapshot_target).map_err(|error| error.to_string())?));
            wakes.push((id, subscription.id.clone()));
            recovery.progress = NameRecoveryProgress::AdjudicationRequested;
            recovery.updated_ms = now;
            events.push(Event::NameRecoveryUpdated { recovery });
        }
    }
    if !events.is_empty() {
        server
            .commit_locked_checked(&mut state, &events)
            .map_err(|error| error.to_string())?;
    }
    drop(state);
    for (id, subscription) in wakes {
        notify(server, &id, &subscription, now);
    }
    Ok(())
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
                subject: Some("名字工作调度与裁决".into()),
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

/// Called by the context owner after parsing the control JSON. Authentication,
/// current-master authority and the occupancy fence are rechecked under lock.
pub(crate) fn adjudicate(
    server: &Server,
    actor: &str,
    token: &str,
    op: &str,
    verdict: NameRecoveryVerdict,
    evidence: &str,
) -> Resp {
    let verdict = if evidence.trim().is_empty() {
        NameRecoveryVerdict::Unknown
    } else {
        verdict
    };
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, actor, token)
        .and_then(|()| board_check_master_locked(server, &state, actor))
    {
        return error;
    }
    let route = match server_route_scope(server, &state) {
        Ok(Some(route)) => route,
        _ => return Resp::err("NAME_ROUTE_REQUIRED"),
    };
    let Some(mut recovery) = state.name_recovery(op).cloned() else {
        return Resp::err("NAME_OPERATION_NOT_FOUND");
    };
    if recovery.project_scope != route.project_scope_id.as_str() {
        return Resp::err("NAME_OPERATION_SCOPE_MISMATCH");
    }
    if recovery.progress == NameRecoveryProgress::StaleDecision {
        return Resp::data(
            json!({"outcome":recovery.progress,"recovery":recovery,"consumed":false,
                "effects":name_resolution::NameResolutionEffects::default()}),
        );
    }
    let now = now_ms();
    let current = state
        .name_occupancy(&recovery.project_scope, &recovery.target_name)
        .cloned();
    if recovery.progress == NameRecoveryProgress::Applied
        && current.as_ref().is_some_and(|occupancy| {
            if recovery.request.invocation == "presence_adjudication" {
                recovery
                    .occupancy_ref
                    .matches(&recovery.project_scope, occupancy)
            } else {
                occupancy.worker_id == recovery.request.caller_worker_id
            }
        })
    {
        return Resp::data(json!({"outcome":"applied","operation_id":op,
                "effects":name_resolution::NameResolutionEffects::from_recovery(&recovery),
                "recovery":recovery,"consumed":false}));
    }
    if current.as_ref().is_some_and(|occupancy| {
        recovery
            .occupancy_ref
            .matches(&recovery.project_scope, occupancy)
    }) && recovery
        .decision
        .as_ref()
        .is_some_and(|decision| decision.verdict == verdict && decision.evidence == evidence)
    {
        return Resp::data(json!({"outcome":if verdict == NameRecoveryVerdict::Online
            && recovery.request.invocation != "presence_adjudication" && !recovery.request.replace_confirmed {
                "needs_confirmation"
            } else { "pending" }, "operation_id":op,"recovery":recovery,"consumed":false,
                "effects":name_resolution::NameResolutionEffects::default()}));
    }
    let mut events = Vec::new();
    let outcome;
    if recovery.progress == NameRecoveryProgress::Applied
        || !current.as_ref().is_some_and(|occupancy| {
            recovery
                .occupancy_ref
                .matches(&recovery.project_scope, occupancy)
        })
    {
        recovery.progress = NameRecoveryProgress::StaleDecision;
        recovery.result = Some(NameRecoveryResult {
            outcome: NameRecoveryOutcome::StaleDecision,
            detail: Some("target occupancy changed".into()),
            recorded_ms: now,
        });
        outcome = "stale_decision";
    } else if verdict == NameRecoveryVerdict::Unknown || evidence.trim().is_empty() {
        recovery.progress = NameRecoveryProgress::AdjudicationRequested;
        recovery.decision = Some(NameRecoveryDecision {
            verdict: NameRecoveryVerdict::Unknown,
            evidence: evidence.into(),
            decided_by: actor.into(),
            decided_ms: now,
        });
        outcome = "pending";
    } else {
        let current = current.unwrap();
        recovery.decision = Some(NameRecoveryDecision {
            verdict,
            evidence: evidence.into(),
            decided_by: actor.into(),
            decided_ms: now,
        });
        events.push(Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: recovery.project_scope.clone(),
                worker_id: current.worker_id.clone(),
                session_id: current.session_id.clone(),
                presence: if verdict == NameRecoveryVerdict::Online {
                    AgentPresence::Online
                } else {
                    AgentPresence::Offline
                },
                reason: format!("Jenny snapshot adjudication {}: {}", op, evidence),
                observed_ms: now,
            },
        });
        let periodic = recovery.request.invocation == "presence_adjudication";
        if !periodic
            && verdict == NameRecoveryVerdict::Online
            && !recovery.request.replace_confirmed
        {
            // The online verdict is evidence, never the caller's confirmation.
            outcome = "needs_confirmation";
            recovery.progress = NameRecoveryProgress::AdjudicationRequested;
        } else {
            if !periodic {
                let Some(worker) = state.workers.get(&recovery.request.caller_worker_id) else {
                    return Resp::err("NAME_CALLER_BINDING_MISSING");
                };
                let Some(binding) = current_worker_binding(
                    &state,
                    &route.project_scope_id,
                    Some(&route.app_scope_id),
                    worker,
                ) else {
                    return Resp::err("NAME_CALLER_BINDING_MISSING");
                };
                let Some(session) = binding.session_id.as_ref() else {
                    return Resp::err("NAME_CALLER_SESSION_MISSING");
                };
                let occupancy = NameOccupancy {
                    name: recovery.target_name.clone(),
                    worker_id: worker.id.clone(),
                    session_id: session.to_string(),
                    occupancy_revision: current.occupancy_revision + 1,
                    updated_ms: now,
                };
                if occupancy.name == "Jenny" {
                    let grant = match master_grant_for_worker(
                        &state,
                        &route,
                        &worker.id,
                        actor,
                        &format!("current Jenny adjudicated authorized pending operation {op}"),
                    ) {
                        Ok(grant) => grant,
                        Err(error) => return Resp::err(error),
                    };
                    events.extend(master_authority_transfer_events(&state, &route, grant));
                }
                events.extend(name_resolution::build_name_resolution_events(
                    &state,
                    &recovery.project_scope,
                    &occupancy,
                ));
            }
            recovery.progress = NameRecoveryProgress::Applied;
            recovery.result = Some(NameRecoveryResult {
                outcome: NameRecoveryOutcome::Applied,
                detail: Some(
                    if periodic {
                        "presence adjudicated"
                    } else {
                        "name and open work inherited"
                    }
                    .into(),
                ),
                recorded_ms: now,
            });
            outcome = "applied";
        }
    }
    recovery.updated_ms = now;
    events.push(Event::NameRecoveryUpdated {
        recovery: recovery.clone(),
    });
    if let Err(error) = server.commit_locked_checked(&mut state, &events) {
        return Resp::err(format!("NAME_ADJUDICATION_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({"outcome":outcome,"operation_id":op,
        "effects":name_resolution::NameResolutionEffects::from_recovery(&recovery),
        "recovery":recovery,"consumed":false}))
}

/// An inconclusive probe cannot erase a known decision for this occupancy.
/// A newer deterministic observation makes the old decision inapplicable.
pub(super) fn retain_adjudicated_presence(state: &State, presence: &AgentPresenceRecord) -> bool {
    if presence.presence != AgentPresence::Unknown {
        return false;
    }
    let Some(old) = state.agent_presence(&presence.project_scope, &presence.worker_id) else {
        return false;
    };
    if old.session_id != presence.session_id || old.presence == AgentPresence::Unknown {
        return false;
    }
    state.name_recoveries.values().any(|recovery| {
        state
            .name_occupancy(&presence.project_scope, &recovery.target_name)
            .is_some_and(|occupancy| {
                occupancy.worker_id == presence.worker_id
                    && recovery
                        .occupancy_ref
                        .matches(&presence.project_scope, occupancy)
            })
            && recovery.decision.as_ref().is_some_and(|decision| {
                decision.verdict != NameRecoveryVerdict::Unknown
                    && decision.decided_ms == old.observed_ms
                    && (decision.verdict == NameRecoveryVerdict::Online)
                        == (old.presence == AgentPresence::Online)
            })
    })
}

#[cfg(test)]
mod tests;
