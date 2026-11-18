//! Pure name decisions and event batches. The context owner validates approval
//! authority, applies any master grant through its existing owner, and commits
//! the entire batch under the state lock. This module does not grant roles.
use super::name_state::{NameOccupancy, TaskAssignment};
use super::{Event, State};

pub(crate) const ORDINARY_NAME_POOL: [&str; 20] = [
    "Alice", "Bob", "Carol", "David", "Emma", "Frank", "Grace", "Henry", "Iris", "Jack", "Kate",
    "Leo", "Mia", "Noah", "Olivia", "Peter", "Quinn", "Rose", "Sam", "Tina",
];

pub(crate) fn display_slot(name: &str) -> Result<Option<u32>, String> {
    if name == "Jenny" { return Ok(Some(1)); }
    ORDINARY_NAME_POOL.iter().position(|candidate| *candidate == name)
        .map(|index| Some(index as u32 + 2))
        .or_else(|| peer_registration_number(name).map(|_| None))
        .ok_or_else(|| format!("NAME_INVALID: unknown registered name {name}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NameIntent {
    pub requested_name: String,
    pub user_authorized: bool,
    /// Legacy handoff policy retained for stored intents. New name and role
    /// operations leave source-name work and mail at their original names.
    pub moves_existing_names: bool,
    /// Typed takeover confirmation for a name held by another carrier. The
    /// legacy promote approval text never sets this; only the explicit typed
    /// `restore_master`/`restore_name` approval object can.
    pub replace_confirmed: bool,
}

/// Recognizes only the two name actions in the existing grant_approval JSON.
/// `decision: approved` expresses user intent; it does not validate authority
/// or replace the existing grant owner's approval checks.
pub(crate) fn parse_name_intent(
    approval: Option<&serde_json::Value>,
) -> Result<Option<NameIntent>, String> {
    let Some(value) = approval else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or("NAME_INTENT_INVALID: grant_approval must be an object")?;
    let action = object.get("action").and_then(serde_json::Value::as_str);
    let name = match action {
        Some("restore_master") => "Jenny",
        Some("restore_name") => object
            .get("requested_name")
            .and_then(serde_json::Value::as_str)
            .ok_or("NAME_INTENT_INVALID: restore_name requires requested_name")?,
        _ => return Ok(None),
    };
    validate_name(name)?;
    let replace_confirmed = match object.get("replace_confirmed") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or("NAME_INTENT_INVALID: replace_confirmed must be boolean")?,
    };
    Ok(Some(NameIntent {
        requested_name: name.into(),
        user_authorized: match object.get("decision") {
            Some(decision) => decision.as_str() == Some("approved"),
            None => object
                .get("authorization")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !text.trim().is_empty()),
        },
        moves_existing_names: false,
        replace_confirmed,
    }))
}

/// Pin legacy work to its known source name before the carrier releases that
/// name. This uses the existing business occupancy selected by the published
/// handoff intent and grants no identity authority.
pub(crate) fn preserve_source_work_events(
    state: &State,
    project_scope: &str,
    source_names: &[String],
    target_name: &str,
    now: i64,
) -> Vec<Event> {
    let mut tasks: Vec<_> = state.tasks.values()
        .filter(|task| !matches!(task.status.as_str(), "closed" | "cancelled")
            && state.task_assignment(&task.id).is_none())
        .collect();
    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    tasks.into_iter().filter_map(|task| {
        let source = source_names.iter()
            .filter(|name| name.as_str() != target_name)
            .filter_map(|name| state.name_occupancy(project_scope, name))
            .find(|source| source.worker_id == task.owner)?;
        Some(Event::TaskAssignmentUpdated { assignment: TaskAssignment {
            task_id: task.id.clone(),
            name: source.name.clone(),
            session_id: source.session_id.clone(),
            assignment_revision: 1,
            reflow: None,
            updated_ms: now,
        } })
    }).collect()
}

/// New registration uses the current map or allocates an available name.
pub(crate) fn name_for_registration(
    state: &State,
    control: &super::binding_slot::BindingControlSet,
    binding: &super::RuntimeBinding,
) -> Result<String, String> {
    if let Some(key) = control.key_for_worker(&binding.project_scope, &binding.app_scope_id, binding.agent_id.as_str())? {
        return Ok(key.name.clone());
    }
    if super::current_master_worker_id(state, Some(&binding.route_scope())).as_deref() == Some(binding.agent_id.as_str()) {
        return Ok("Jenny".into());
    }
    control.ordinary_name(&binding.project_scope, &binding.app_scope_id, binding.agent_id.as_str())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name == "Jenny"
        || ORDINARY_NAME_POOL.contains(&name)
        || peer_registration_number(name).is_some()
    {
        Ok(())
    } else {
        Err(format!(
            "NAME_INVALID: {name}; expected Jenny, a fixed-pool name, or peer-<number>"
        ))
    }
}

pub(crate) fn peer_registration_number(name: &str) -> Option<u64> {
    let number = name.strip_prefix("peer-")?.parse::<u64>().ok()?;
    (number > 0 && format!("peer-{number}") == name).then_some(number)
}

/// Build one transaction after a successful decision. `rename_from` carries
/// the worker's previous names only when the caller explicitly renamed that
/// worker. Automatic session/pane inheritance keeps the target-name handoff
/// alone; a fresh ordinary name with no anchor must not move the old name's
/// tasks or mailbox.
pub(crate) fn build_name_resolution_events_with_rename_from(
    state: &State,
    project_scope: &str,
    occupancy: &NameOccupancy,
    rename_from: &[String],
    previous: Option<&NameOccupancy>,
) -> Vec<Event> {
    // A fresh ordinary name has no previous holder. The caller worker id is
    // not a legacy identity owner in that case; only the target name's prior
    // holder or an explicit rename source can authorize a task handoff.
    let legacy_owner = previous.map(|holder| holder.worker_id.as_str());
    let target_held_by_other = previous.is_some_and(|holder| {
        holder.worker_id != occupancy.worker_id || holder.session_id != occupancy.session_id
    });
    let mut tasks: Vec<_> = state.tasks.values().collect();
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    let mut assignments = Vec::new();
    let mut events = Vec::new();
    for task in tasks {
        if matches!(task.status.as_str(), "closed" | "cancelled") {
            continue;
        }
        let existing = state.task_assignment(&task.id);
        let matches_rename_from = existing
            .is_some_and(|assignment| rename_from.iter().any(|name| name == &assignment.name));
        // A task already addressed to this name follows the name to its
        // current carrier. The previous-holder check would miss a release and
        // later re-take by a different session, but a same-session commit is
        // still a no-op.
        let matches_target_name = existing.is_some_and(|assignment| {
            assignment.name == occupancy.name
                && (target_held_by_other || assignment.session_id != occupancy.session_id)
        });
        let matches_caller_name = matches_rename_from || matches_target_name;
        let matches_legacy_owner =
            existing.is_none() && legacy_owner.is_some_and(|owner| task.owner == owner);
        if !matches_caller_name && !matches_legacy_owner {
            continue;
        }
        let assignment_changed = existing
            .is_none_or(|a| a.name != occupancy.name || a.session_id != occupancy.session_id);
        let owner_changed = task.owner != occupancy.worker_id;
        if !assignment_changed && !owner_changed {
            continue;
        }
        let mut assignment = existing.cloned().unwrap_or_else(|| TaskAssignment {
            task_id: task.id.clone(),
            name: occupancy.name.clone(),
            session_id: occupancy.session_id.clone(),
            assignment_revision: 0,
            reflow: None,
            updated_ms: occupancy.updated_ms,
        });
        assignment.name = occupancy.name.clone();
        assignment.session_id = occupancy.session_id.clone();
        assignment.assignment_revision += 1;
        assignment.updated_ms = occupancy.updated_ms;
        assignments.push(assignment);
        if owner_changed {
            let mut updated = task.clone();
            updated.owner = occupancy.worker_id.clone();
            updated.updated_ms = occupancy.updated_ms;
            events.push(Event::TaskUpdated { task: updated });
            events.extend(task_resource_owner_events(
                state,
                task,
                &occupancy.worker_id,
            ));
        }
        let mut details = state
            .board_details
            .get(&task.id)
            .cloned()
            .unwrap_or_else(|| crate::board::BoardTaskDetails::legacy(&task.id, false));
        details.revision += 1;
        events.push(Event::BoardDetailsChanged {
            task_id: task.id.clone(),
            details,
        });
    }
    let releases_extra_names = state
            .name_occupancies
            .get(project_scope)
            .is_some_and(|names| {
                names.values().any(|holder| {
                    holder.worker_id == occupancy.worker_id
                        && holder.session_id == occupancy.session_id
                        && holder.name != occupancy.name
                })
            });
    if previous != Some(occupancy) || !assignments.is_empty() || releases_extra_names {
        events.insert(
            0,
            Event::NameResolutionCommitted {
                project_scope: project_scope.into(),
                occupancy: occupancy.clone(),
                assignments,
            },
        );
    }
    events.extend(super::name_mailbox::handoff_events(
        state,
        project_scope,
        occupancy,
        rename_from,
        previous,
    ));
    // The name carries its finite goal schedules to the new execution carrier.
    // Reuse the subscription record/event so IDs, timing and terminal state stay
    // intact, and the normal worker authorization still rejects the old caller.
    let previous_changed = previous.as_ref().is_some_and(|previous| {
        previous.worker_id != occupancy.worker_id || previous.session_id != occupancy.session_id
    });
    let mut handoff_worker_ids: Vec<String> = Vec::new();
    if previous_changed {
        if let Some(previous) = previous {
            handoff_worker_ids.push(previous.worker_id.clone());
        }
    }
    handoff_worker_ids.sort();
    handoff_worker_ids.dedup();
    if !handoff_worker_ids.is_empty() {
        if let Some(transport) = state
            .workers
            .get(&occupancy.worker_id)
            .and_then(super::selected_transport_for_worker)
        {
            if let Some(target) = super::notification_target(&transport) {
                let mut subscriptions: Vec<_> = state
                    .notification_subscriptions
                    .values()
                    .filter(|s| {
                        if !handoff_worker_ids
                            .iter()
                            .any(|worker_id| *worker_id == s.worker_id)
                        {
                            return false;
                        }
                        let goal_deadline = s.event == "deadline"
                            && s.subject
                                .as_deref()
                                .is_some_and(|subject| subject.starts_with("goal:"))
                            && matches!(s.status.as_str(), "armed" | "transport-lost" | "consumed");
                        let project_patrol = super::is_exact_project_patrol(s)
                            && matches!(s.status.as_str(), "armed" | "cancelled");
                        let not_expired =
                            s.status == "cancelled" || s.expires_ms > occupancy.updated_ms;
                        (goal_deadline || project_patrol) && not_expired
                    })
                    .cloned()
                    .collect();
                subscriptions.sort_by(|a, b| a.id.cmp(&b.id));
                let already_handed_off: std::collections::BTreeSet<String> = events
                    .iter()
                    .filter_map(|event| match event {
                        Event::MailboxReassigned { handoff } => Some(handoff.message_id.clone()),
                        _ => None,
                    })
                    .collect();
                for mut subscription in subscriptions {
                    subscription.worker_id = occupancy.worker_id.clone();
                    subscription.method = transport.kind.as_str().into();
                    subscription.target = target.as_ref().to_owned();
                    subscription.updated_ms = occupancy.updated_ms;
                    if subscription.status == "transport-lost" {
                        subscription.status = "armed".into();
                        subscription.status_reason = None;
                    }
                    // A goal timer message can already be pending when the
                    // session commits its next ordinary name. That message has
                    // no name address, so the name mailbox selector above
                    // cannot find it. Follow the subscription's own wake
                    // binding and move the message with its schedule.
                    let mut timer_handoffs = Vec::new();
                    for (message_id, bound) in state.wake_bindings.iter() {
                        if bound != &subscription.id || already_handed_off.contains(message_id) {
                            continue;
                        }
                        let Some(message) = state.msgs.get(message_id) else {
                            continue;
                        };
                        if !matches!(message.state.as_str(), "pending" | "delivered")
                            || message.to == occupancy.worker_id
                            || !handoff_worker_ids
                                .iter()
                                .any(|worker_id| *worker_id == message.to)
                        {
                            continue;
                        }
                        timer_handoffs.push(Event::MailboxReassigned {
                            handoff: super::name_mailbox::MailboxHandoff {
                                project_scope: project_scope.into(),
                                name: occupancy.name.clone(),
                                occupancy_revision: occupancy.occupancy_revision,
                                session_id: occupancy.session_id.clone(),
                                message_id: message.id.clone(),
                                previous_recipient: message.to.clone(),
                                recipient: occupancy.worker_id.clone(),
                                wake_attempt_count: message.wake_attempt_count,
                                previous_subscription: Some(subscription.id.clone()),
                                accepted_ms: state
                                    .notification_delivery_accepted
                                    .get(&message.id)
                                    .copied(),
                                evidence: state
                                    .notification_delivery_evidence
                                    .get(&message.id)
                                    .cloned(),
                                failure: state
                                    .notification_delivery_failures
                                    .get(&message.id)
                                    .cloned(),
                            },
                        });
                    }
                    events.extend(timer_handoffs);
                    // A deadline already in the mailbox must still consume its
                    // own schedule after handoff, rather than the direct inbox.
                    let bindings: Vec<_> = events
                        .iter()
                        .filter_map(|event| match event {
                            Event::MailboxReassigned { handoff }
                                if subscription.status == "armed"
                                    && handoff.previous_subscription.as_deref()
                                        == Some(subscription.id.as_str()) =>
                            {
                                Some(Event::WakeBound {
                                    message_id: handoff.message_id.clone(),
                                    subscription_id: subscription.id.clone(),
                                })
                            }
                            _ => None,
                        })
                        .collect();
                    events.push(Event::NotificationSubscribed { subscription });
                    events.extend(bindings);
                }
            }
        }
    }
    events
}

/// Move only references already owned by this task's current holder.
pub(crate) fn task_resource_owner_events(
    state: &State,
    task: &super::state::TaskRec,
    new_owner: &str,
) -> Vec<Event> {
    let mut events = Vec::new();
    let mut resources: Vec<_> = state
        .worktree_bindings
        .values()
        .filter(|binding| binding.task_id == task.id && binding.owner_agent_id == task.owner)
        .cloned()
        .collect();
    resources.sort_by(|a, b| a.worktree_root.cmp(&b.worktree_root));
    for mut binding in resources {
        binding.owner_agent_id = new_owner.into();
        events.push(Event::WorktreeBound { binding });
    }
    if let Some(request) = state
        .pending_merges
        .get(&task.id)
        .filter(|request| request.owner == task.owner)
    {
        let mut request = request.clone();
        request.owner = new_owner.into();
        events.push(Event::MergeRequested { request });
    }
    events
}
