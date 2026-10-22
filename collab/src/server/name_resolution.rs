//! Pure name decisions and event batches. The context owner validates approval
//! authority, applies any master grant through its existing owner, and commits
//! the entire batch under the state lock. This module does not grant roles.
use super::name_state::{
    AgentPresence, NameOccupancy, NameRecoveryPending, NameRecoveryProgress, NameRecoveryRequest,
    TaskAssignment,
};
use super::{Event, State};

pub(crate) const ORDINARY_NAME_POOL: [&str; 20] = [
    "Alice", "Bob", "Carol", "David", "Emma", "Frank", "Grace", "Henry", "Iris", "Jack", "Kate",
    "Leo", "Mia", "Noah", "Olivia", "Peter", "Quinn", "Rose", "Sam", "Tina",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NameIntent {
    pub requested_name: String,
    pub user_authorized: bool,
    pub replace_confirmed: bool,
}

/// Control receipts from the name owner's successful atomic commit.
#[derive(Debug, Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) struct NameResolutionEffects {
    pub grant_committed: bool,
}

impl NameResolutionEffects {
    pub(crate) fn from_recovery(recovery: &NameRecoveryPending) -> Self {
        Self {
            // This persisted application was committed in the same batch as
            // the Jenny grant. Presence-only adjudication does not grant roles.
            grant_committed: recovery.progress == NameRecoveryProgress::Applied
                && recovery.request.invocation != "presence_adjudication"
                && recovery.target_name == "Jenny"
                && recovery.result.as_ref().is_some_and(|result| {
                    result.outcome == super::name_state::NameRecoveryOutcome::Applied
                }),
        }
    }
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
        user_authorized: object.get("decision").and_then(serde_json::Value::as_str)
            == Some("approved")
            || object
                .get("authorization")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !text.trim().is_empty()),
        replace_confirmed,
    }))
}

/// The admitted registration/context owner calls this only after its binding
/// exists. Role and name effects share one checked batch; a pending request
/// retains its original facts for Jenny's continuation owner.
pub(crate) fn resolve_registered_name(
    server: &super::Server,
    worker_id: &str,
    operation_id: &str,
    invocation: &str,
    intent: Option<NameIntent>,
) -> Result<serde_json::Value, String> {
    resolve_registered_name_with_authorization(
        server,
        worker_id,
        operation_id,
        invocation,
        intent,
        None,
    )
}

pub(crate) fn resolve_registered_name_with_authorization(
    server: &super::Server,
    worker_id: &str,
    operation_id: &str,
    invocation: &str,
    intent: Option<NameIntent>,
    authorization: Option<&str>,
) -> Result<serde_json::Value, String> {
    use super::name_state::NameBindingReference;
    let mut state = server.state.lock().unwrap();
    let route = super::server_route_scope(server, &state)?
        .ok_or("NAME_IDENTITY_REQUIRED: registered project route required")?;
    let binding = state
        .global
        .lookup_project_for_route(&route)
        .and_then(|project| {
            project
                .runtime_bindings
                .values()
                .find(|binding| binding.agent_id.as_str() == worker_id)
        })
        .ok_or("NAME_IDENTITY_REQUIRED: current runtime binding required")?;
    let session = binding
        .session_id
        .as_ref()
        .ok_or("NAME_IDENTITY_REQUIRED: agent session required")?
        .to_string();
    let request = NameRecoveryRequest {
        invocation: invocation.into(),
        requested_name: intent.as_ref().map(|intent| intent.requested_name.clone()),
        caller_worker_id: worker_id.into(),
        caller_session_id: session,
        caller_binding: NameBindingReference {
            app_scope_id: binding.app_scope_id.to_string(),
            binding_id: binding.binding_id.to_string(),
            endpoint_generation: binding.endpoint_generation,
        },
        replace_confirmed: intent
            .as_ref()
            .is_some_and(|intent| intent.replace_confirmed),
        requested_ms: super::now_ms(),
    };
    if let Some(saved) = state.name_recovery(operation_id) {
        if saved.project_scope != route.project_scope_id.as_str()
            || saved.request.caller_worker_id != worker_id
            || saved.request.requested_name != request.requested_name
            || saved.request.replace_confirmed != request.replace_confirmed
        {
            return Err("NAME_OPERATION_CONFLICT: resume the saved name recovery operation".into());
        }
        if saved.progress == NameRecoveryProgress::Applied {
            let occupancy = state
                .name_occupancy(&saved.project_scope, &saved.target_name)
                .filter(|holder| holder.worker_id == worker_id)
                .ok_or("NAME_OPERATION_CONFLICT: applied name belongs to another worker")?;
            return Ok(
                serde_json::json!({"outcome":"committed", "operation_id":operation_id,
                "name":occupancy.name, "session_id":occupancy.session_id, "recovery":saved,
                "effects":NameResolutionEffects::from_recovery(saved)}),
            );
        }
        // Keep the original timestamp and invocation. Do not restart an
        // adjudicated operation against a newer occupancy.
        return Ok(
            serde_json::json!({"outcome": saved.progress, "recovery": saved,
            "operation_id": operation_id, "effects":NameResolutionEffects::default()}),
        );
    }
    let is_master =
        super::current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker_id);
    let input = NameResolutionInput {
        project_scope: route.project_scope_id.as_str().to_owned(),
        operation_id: operation_id.into(),
        request,
        intent,
        is_current_master: is_master,
    };
    let plan = plan_name_resolution(&state, &input)?;
    let mut events = Vec::new();
    let mut outcome = match &plan.decision {
        NameResolutionDecision::Committed {
            occupancy,
            synced_tasks,
        } => {
            if plan.requires_master_authorization {
                if !input
                    .intent
                    .as_ref()
                    .is_some_and(|intent| intent.user_authorized)
                {
                    return Err(
                        "NAME_AUTHORIZATION_REQUIRED: Jenny requires user authorization".into(),
                    );
                }
                let grant = super::master_grant_for_worker(
                    &state,
                    &route,
                    worker_id,
                    "user",
                    authorization
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or("explicit context name recovery authorization"),
                )?;
                events.extend(super::master_authority_transfer_events(
                    &state, &route, grant,
                ));
            } else if is_master && occupancy.name != "Jenny" {
                events.extend(super::master_authority_revoke_events(&state, &route));
            }
            serde_json::json!({"outcome":"committed", "name":occupancy.name,
                "session_id":occupancy.session_id, "synced_tasks":synced_tasks,
                "operation_id":operation_id})
        }
        NameResolutionDecision::NeedsConfirmation { occupancy } => {
            let approval = serde_json::json!({"action":"restore_name", "requested_name":occupancy.name,
                "authorization":"<user authorization>", "replace_confirmed":true});
            serde_json::json!({"outcome":"confirmation_required", "occupancy":occupancy,
                "operation_id":operation_id,
                "next_invocation":format!("collab context --approve-grant '{approval}'")})
        }
        NameResolutionDecision::Pending { recovery } => serde_json::json!({
            "outcome":"pending", "recovery":recovery, "operation_id":operation_id}),
    };
    let effects = NameResolutionEffects {
        grant_committed: events.iter().any(|event| {
            matches!(
                event,
                Event::GlobalMasterGranted { .. } | Event::GlobalMasterRevoked { .. }
            )
        }),
    };
    events.extend(plan.events);
    if !events.is_empty() {
        server
            .commit_locked_checked(&mut state, &events)
            .map_err(|error| format!("NAME_DURABILITY_FAILED: {error}"))?;
    }
    outcome["effects"] = serde_json::to_value(effects).expect("name effects serialize");
    Ok(outcome)
}

/// Pure readback for query and same-key retries. Never allocates or rebinds.
pub(crate) fn project_registered_name(
    server: &super::Server,
    worker_id: &str,
    snapshot: &mut serde_json::Value,
) {
    let state = server.state.lock().unwrap();
    let Ok(Some(route)) = super::server_route_scope(server, &state) else {
        return;
    };
    if let Some(holder) = state
        .name_occupancies
        .get(route.project_scope_id.as_str())
        .and_then(|names| names.values().find(|holder| holder.worker_id == worker_id))
    {
        snapshot["name"] = serde_json::json!(holder.name);
        snapshot["session_id"] = serde_json::json!(holder.session_id);
        snapshot["identity"]["display_name"] = serde_json::json!(holder.name);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NameResolutionInput {
    pub project_scope: String,
    /// Existing context operation ID, also used for durable pending recovery.
    pub operation_id: String,
    pub request: NameRecoveryRequest,
    pub intent: Option<NameIntent>,
    /// Read from the authoritative role owner, never inferred from a label.
    pub is_current_master: bool,
}

#[derive(Debug)]
pub(crate) enum NameResolutionDecision {
    Committed {
        occupancy: NameOccupancy,
        synced_tasks: usize,
    },
    NeedsConfirmation {
        occupancy: NameOccupancy,
    },
    Pending {
        recovery: NameRecoveryPending,
    },
}

#[derive(Debug)]
pub(crate) struct NameResolutionPlan {
    pub decision: NameResolutionDecision,
    pub events: Vec<Event>,
    /// The context owner must obtain the existing master's grant before commit.
    pub requires_master_authorization: bool,
}

fn validate_name(name: &str) -> Result<(), String> {
    if name == "Jenny" || ORDINARY_NAME_POOL.contains(&name) {
        Ok(())
    } else {
        Err(format!(
            "NAME_INVALID: {name}; expected Jenny or a fixed-pool name"
        ))
    }
}

fn holder_presence(state: &State, project: &str, holder: &NameOccupancy) -> AgentPresence {
    state
        .agent_presence(project, &holder.worker_id)
        .filter(|record| record.session_id == holder.session_id)
        .map(|record| record.presence)
        .unwrap_or(AgentPresence::Unknown)
}

pub(crate) fn available_ordinary_name(
    state: &State,
    project: &str,
    worker: &str,
) -> Result<String, String> {
    // Recovery preserves an ordinary work identity. Pool order only chooses a
    // name when this carrier has none, such as a transition away from Jenny.
    if let Some(name) = ORDINARY_NAME_POOL.iter().find(|name| {
        state.name_occupancy(project, name)
            .is_some_and(|holder| holder.worker_id == worker)
    }) {
        return Ok((*name).to_owned());
    }
    ORDINARY_NAME_POOL
        .iter()
        .find(|name| {
            state.name_occupancy(project, name).is_none_or(|holder| {
                holder.worker_id == worker
                    || holder_presence(state, project, holder) == AgentPresence::Offline
            })
        })
        .map(|name| (*name).to_owned())
        .ok_or_else(|| "NAME_POOL_FULL: no ordinary name available for the former Jenny".into())
}

/// Call with the current locked State and an admitted stable worker ID. No
/// transport, historical label, activity, or old-session gate decides ownership.
pub(crate) fn plan_name_resolution(
    state: &State,
    input: &NameResolutionInput,
) -> Result<NameResolutionPlan, String> {
    let request = &input.request;
    if input.project_scope.trim().is_empty()
        || input.operation_id.trim().is_empty()
        || request.caller_worker_id.trim().is_empty()
        || request.caller_session_id.trim().is_empty()
    {
        return Err(
            "NAME_IDENTITY_REQUIRED: project, operation, worker and agent session are required"
                .into(),
        );
    }
    if let Some(intent) = &input.intent {
        validate_name(&intent.requested_name)?;
        if !intent.user_authorized {
            return Err(
                "NAME_AUTHORIZATION_REQUIRED: explicit name intent needs user approval".into(),
            );
        }
    }
    // Raw recovery fields alone are not an authorization to choose a name.
    if input.intent.is_none() && (request.requested_name.is_some() || request.replace_confirmed) {
        return Err("NAME_AUTHORIZATION_REQUIRED: requested name needs typed name intent".into());
    }
    let names = state.name_occupancies.get(&input.project_scope);
    let current = names.and_then(|names| {
        names
            .values()
            .find(|holder| holder.name != "Jenny" && holder.worker_id == request.caller_worker_id)
    });
    let same_session = names.and_then(|names| {
        names
            .values()
            .find(|holder| holder.name != "Jenny" && holder.session_id == request.caller_session_id)
    });
    let target = if let Some(intent) = &input.intent {
        intent.requested_name.clone()
    } else if input.is_current_master {
        "Jenny".into()
    } else if let Some(holder) = current.or(same_session) {
        holder.name.clone()
    } else {
        ORDINARY_NAME_POOL
            .iter()
            .find(|name| {
                state
                    .name_occupancy(&input.project_scope, name)
                    .map(|holder| {
                        holder_presence(state, &input.project_scope, holder)
                            == AgentPresence::Offline
                    })
                    .unwrap_or(true)
            })
            .ok_or("NAME_POOL_FULL: all 20 ordinary project names are occupied; Jenny is reserved")?
            .to_string()
    };
    let requires_master_authorization = target == "Jenny" && !input.is_current_master;
    let previous = state.name_occupancy(&input.project_scope, &target);
    if let Some(holder) = previous {
        if holder.worker_id != request.caller_worker_id
            && holder.session_id != request.caller_session_id
        {
            match holder_presence(state, &input.project_scope, holder) {
                AgentPresence::Online
                    if !input
                        .intent
                        .as_ref()
                        .is_some_and(|intent| intent.replace_confirmed) =>
                {
                    return Ok(NameResolutionPlan {
                        decision: NameResolutionDecision::NeedsConfirmation {
                            occupancy: holder.clone(),
                        },
                        events: vec![],
                        requires_master_authorization,
                    });
                }
                AgentPresence::Unknown
                    if target == "Jenny"
                        && input.intent.as_ref().is_some_and(|intent| {
                            intent.user_authorized && intent.replace_confirmed
                        }) => {}
                AgentPresence::Unknown => {
                    let mut original = request.clone();
                    original.requested_name = Some(target.clone());
                    original.replace_confirmed = input
                        .intent
                        .as_ref()
                        .is_some_and(|intent| intent.replace_confirmed);
                    let recovery = NameRecoveryPending {
                        operation_id: input.operation_id.clone(),
                        project_scope: input.project_scope.clone(),
                        target_name: target,
                        occupancy_ref: holder.reference(&input.project_scope),
                        request: original,
                        progress: NameRecoveryProgress::Pending,
                        decision: None,
                        result: None,
                        created_ms: request.requested_ms,
                        updated_ms: request.requested_ms,
                    };
                    // Repeated calls with the same operation preserve adjudication progress.
                    let recovery = match state.name_recovery(&input.operation_id) {
                        Some(existing) if existing.occupancy_ref == recovery.occupancy_ref && existing.request == recovery.request => existing.clone(),
                        Some(_) => return Err("NAME_OPERATION_CONFLICT: resume the existing recovery against its saved occupancy".into()),
                        None => recovery,
                    };
                    let events = if state.name_recovery(&input.operation_id) == Some(&recovery) {
                        vec![]
                    } else {
                        vec![Event::NameRecoveryUpdated {
                            recovery: recovery.clone(),
                        }]
                    };
                    return Ok(NameResolutionPlan {
                        decision: NameResolutionDecision::Pending { recovery },
                        events,
                        requires_master_authorization,
                    });
                }
                _ => {}
            }
        }
    }
    let occupancy = match previous {
        Some(holder)
            if holder.worker_id == request.caller_worker_id
                && holder.session_id == request.caller_session_id =>
        {
            holder.clone()
        }
        _ => NameOccupancy {
            name: target.clone(),
            worker_id: request.caller_worker_id.clone(),
            session_id: request.caller_session_id.clone(),
            occupancy_revision: state
                .name_occupancy_revision(&input.project_scope, &target)
                .checked_add(1)
                .ok_or("NAME_REVISION_EXHAUSTED")?,
            updated_ms: request.requested_ms,
        },
    };
    let events = build_name_resolution_events(state, &input.project_scope, &occupancy);
    let synced_tasks = events
        .iter()
        .find_map(|event| match event {
            Event::NameResolutionCommitted { assignments, .. } => Some(assignments.len()),
            _ => None,
        })
        .unwrap_or(0);
    Ok(NameResolutionPlan {
        decision: NameResolutionDecision::Committed {
            occupancy,
            synced_tasks,
        },
        events,
        requires_master_authorization,
    })
}

/// Build one transaction after a successful decision. State is project-local;
/// explicit task assignments override legacy TaskRec.owner matching.
pub(crate) fn build_name_resolution_events(
    state: &State,
    project_scope: &str,
    occupancy: &NameOccupancy,
) -> Vec<Event> {
    let previous = state.name_occupancy(project_scope, &occupancy.name);
    let legacy_owner = previous
        .map(|holder| holder.worker_id.as_str())
        .unwrap_or(&occupancy.worker_id);
    let mut tasks: Vec<_> = state.tasks.values().collect();
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    let mut assignments = Vec::new();
    let mut events = Vec::new();
    for task in tasks {
        if matches!(task.status.as_str(), "closed" | "cancelled") {
            continue;
        }
        let existing = state.task_assignment(&task.id);
        if !existing
            .map(|a| a.name == occupancy.name)
            .unwrap_or(task.owner == legacy_owner)
        {
            continue;
        }
        let assignment_changed = existing.is_none_or(|a| a.session_id != occupancy.session_id);
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
    if previous != Some(occupancy) || !assignments.is_empty() {
        events.insert(
            0,
            Event::NameResolutionCommitted {
                project_scope: project_scope.into(),
                occupancy: occupancy.clone(),
                assignments,
            },
        );
    }
    if state.worker_names.get(&occupancy.worker_id) != Some(&occupancy.name) {
        events.push(Event::WorkerNamed {
            worker_id: occupancy.worker_id.clone(),
            name: Some(occupancy.name.clone()),
        });
    }
    events.extend(super::name_mailbox::handoff_events(
        state,
        project_scope,
        occupancy,
    ));
    // The name carries its finite goal schedules to the new execution carrier.
    // Reuse the subscription record/event so IDs, timing and terminal state stay
    // intact, and the normal worker authorization still rejects the old caller.
    if let Some(previous) = previous.filter(|previous| {
        previous.worker_id != occupancy.worker_id || previous.session_id != occupancy.session_id
    }) {
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
                        s.worker_id == previous.worker_id
                            && s.event == "deadline"
                            && s.subject
                                .as_deref()
                                .is_some_and(|subject| subject.starts_with("goal:"))
                            && matches!(s.status.as_str(), "armed" | "transport-lost" | "consumed")
                            && s.expires_ms > occupancy.updated_ms
                    })
                    .cloned()
                    .collect();
                subscriptions.sort_by(|a, b| a.id.cmp(&b.id));
                for mut subscription in subscriptions {
                    subscription.worker_id = occupancy.worker_id.clone();
                    subscription.method = transport.kind.as_str().into();
                    subscription.target = target.into();
                    subscription.updated_ms = occupancy.updated_ms;
                    if subscription.status == "transport-lost" {
                        subscription.status = "armed".into();
                        subscription.status_reason = None;
                    }
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
