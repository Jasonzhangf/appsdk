//! Timer-owned task scheduling reflow.
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
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load()?;
    let mut state = server.state.lock().unwrap();
    if state.admission_frozen() {
        return Ok(());
    }
    let Some(route) = server_route_scope(server, &state)? else {
        return Ok(());
    };
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
