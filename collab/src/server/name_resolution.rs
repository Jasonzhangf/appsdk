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
    /// A user-requested name change carries the caller's previous names with it.
    /// Role recovery that allocates an ordinary name for the same carrier is not
    /// a rename and must leave existing name work and mail at the old name.
    pub moves_existing_names: bool,
    /// Typed takeover confirmation for a name held by another carrier. The
    /// legacy promote approval text never sets this; only the explicit typed
    /// `restore_master`/`restore_name` approval object can.
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
        user_authorized: match object.get("decision") {
            Some(decision) => decision.as_str() == Some("approved"),
            None => object
                .get("authorization")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !text.trim().is_empty()),
        },
        moves_existing_names: true,
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
    pane_anchor: Option<super::name_state::NamePaneAnchor>,
) -> Result<serde_json::Value, String> {
    let mut state = server.state.lock().unwrap();
    let (mut outcome, notices) = resolve_registered_name_locked(
        server,
        &mut state,
        worker_id,
        operation_id,
        invocation,
        intent,
        authorization,
        pane_anchor,
    )?;
    drop(state);
    outcome["notifications"] =
        serde_json::json!(super::deliver_relationship_notices(server, &notices));
    Ok(outcome)
}

/// Core business name/handoff commit for a caller that already holds the
/// project-state lock. Sharing that single lock with the control-slot commit
/// is what keeps the preplayed name, the published control slot and the
/// business occupancy on one snapshot; the public wrapper above and the
/// control-first context path both build on this function.
pub(crate) fn resolve_registered_name_locked(
    server: &super::Server,
    state: &mut State,
    worker_id: &str,
    operation_id: &str,
    invocation: &str,
    intent: Option<NameIntent>,
    authorization: Option<&str>,
    pane_anchor: Option<super::name_state::NamePaneAnchor>,
) -> Result<(serde_json::Value, Vec<Event>), String> {
    use super::name_state::NameBindingReference;
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
    let is_master =
        super::current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker_id);
    let input = NameResolutionInput {
        project_scope: route.project_scope_id.as_str().to_owned(),
        operation_id: operation_id.into(),
        request,
        intent,
        is_current_master: is_master,
        pane_anchor,
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
    let mut notices = Vec::new();
    if let NameResolutionDecision::Committed { occupancy, .. } = &plan.decision {
        if !is_master
            && occupancy.name != "Jenny"
            && state
                .name_occupancy(route.project_scope_id.as_str(), &occupancy.name)
                .is_none()
        {
            if let Some(master) = super::current_master_worker_id(&state, Some(&route))
                .filter(|master| master != worker_id && state.workers.contains_key(master))
            {
                notices = super::relationship_notice_events(&state, route.project_scope_id.as_str(), &master,
                    "peer-registered", format!("新 peer {} 已加入。读取 collab board show 核对当前任务与可用人选；有已授权且可交付的子目标时，沿基础 master/peer 模板 publish 后 dispatch 或 invite。无需先设置专长或策略；没有可派任务则保留收件，不创建空转巡检。", occupancy.name));
            }
        }
    }
    events.extend(plan.events);
    events.extend(notices.clone());
    if !events.is_empty() {
        server
            .commit_locked_checked(&mut *state, &events)
            .map_err(|error| format!("NAME_DURABILITY_FAILED: {error}"))?;
    }
    outcome["effects"] = serde_json::to_value(effects).expect("name effects serialize");
    Ok((outcome, notices))
}

/// Decide or preserve the worker's stable name for a binding-control commit.
///
/// The control resource keys a slot by canonical project plus stable name, so
/// a refresh/recovery must resolve the name before publishing the binding. An
/// identity that already carries a name keeps it without asking for one. An
/// identity without any name is assigned the ordinary name the name planner
/// chooses from the same real current facts used by registration: the current
/// agent session, the observed pane anchor (none on the transport-only path),
/// and the fixed pool. This function never scans a project journal to discover
/// a binding.
pub(crate) fn ensure_worker_control_name(
    server: &super::Server,
    worker_id: &str,
) -> Result<String, String> {
    use super::name_state::NameBindingReference;

    let mut state = server.state.lock().unwrap();
    let route = super::server_route_scope(server, &state)?
        .ok_or("NAME_IDENTITY_REQUIRED: registered project route required")?;
    let project_scope = route.project_scope_id.as_str().to_owned();
    if let Some(holder) = state
        .name_occupancies
        .get(&project_scope)
        .and_then(|names| names.values().find(|holder| holder.worker_id == worker_id))
    {
        return Ok(holder.name.clone());
    }

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
        invocation: "transport_refresh".into(),
        requested_name: None,
        caller_worker_id: worker_id.into(),
        caller_session_id: session,
        caller_binding: NameBindingReference {
            app_scope_id: binding.app_scope_id.to_string(),
            binding_id: binding.binding_id.to_string(),
            endpoint_generation: binding.endpoint_generation,
        },
        replace_confirmed: false,
        requested_ms: super::now_ms(),
    };
    let is_master =
        super::current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker_id);
    let input = NameResolutionInput {
        project_scope,
        operation_id: format!("control-{}", binding.binding_id),
        request,
        intent: None,
        is_current_master: is_master,
        pane_anchor: None,
    };
    let plan = plan_name_resolution(&state, &input)?;
    let occupancy = match &plan.decision {
        NameResolutionDecision::Committed { occupancy, .. } => occupancy.clone(),
        NameResolutionDecision::Pending { .. } => {
            return Err("NAME_PENDING: name decision requires user authorization".into());
        }
    };
    let name = occupancy.name.clone();
    if !plan.events.is_empty() {
        server
            .commit_locked_checked(&mut state, &plan.events)
            .map_err(|error| format!("NAME_DURABILITY_FAILED: {error}"))?;
    }
    Ok(name)
}

/// Read the name that the same commit path will assign without committing it.
/// The context owner uses this to rename the tmux session before the name
/// transaction, so a rename failure leaves the old identity unchanged.
pub(crate) fn preview_registered_name(
    server: &super::Server,
    worker_id: &str,
    operation_id: &str,
    invocation: &str,
    intent: Option<NameIntent>,
    pane_anchor: Option<super::name_state::NamePaneAnchor>,
) -> Result<Option<String>, String> {
    use super::name_state::NameBindingReference;
    let state = server.state.lock().unwrap();
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
        return Ok((saved.progress == NameRecoveryProgress::Applied)
            .then(|| saved.target_name.clone()));
    }
    let is_master =
        super::current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker_id);
    let input = NameResolutionInput {
        project_scope: route.project_scope_id.as_str().to_owned(),
        operation_id: operation_id.into(),
        request,
        intent,
        is_current_master: is_master,
        pane_anchor,
    };
    let plan = plan_name_resolution(&state, &input)?;
    match &plan.decision {
        NameResolutionDecision::Committed { occupancy, .. }
            if !plan.requires_master_authorization
                || input
                    .intent
                    .as_ref()
                    .is_some_and(|intent| intent.user_authorized) =>
        {
            Ok(Some(occupancy.name.clone()))
        }
        _ => Ok(None),
    }
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
    /// The caller's currently observed pane and exact invocation cwd. Only the
    /// third name anchor; never a stored identity or historical scan.
    pub pane_anchor: Option<super::name_state::NamePaneAnchor>,
}

#[derive(Debug)]
pub(crate) enum NameResolutionDecision {
    Committed {
        occupancy: NameOccupancy,
        synced_tasks: usize,
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

/// A committed explicit name move, expressed in the terms the binding-control
/// owner needs for one atomic control commit: the target stable name and the
/// caller's previous names that must be retired at the same time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedNameMove {
    pub(crate) target: String,
    pub(crate) source_names: Vec<String>,
}

/// Read-only planning for an explicit name change. It returns the control move
/// only when the explicit decision commits a real carrier change. Pending
/// decisions, Jenny takeovers and no-op repeats return `None`; the caller then
/// keeps the existing name-resolution commit flow unchanged. This function
/// never persists anything and never scans a journal.
pub(crate) fn plan_explicit_name_move(
    server: &super::Server,
    worker_id: &str,
    operation_id: &str,
    invocation: &str,
    intent: &NameIntent,
    pane_anchor: Option<super::name_state::NamePaneAnchor>,
) -> Result<Option<PlannedNameMove>, String> {
    let state = server.state.lock().unwrap();
    plan_explicit_name_move_locked(
        server,
        &state,
        worker_id,
        operation_id,
        invocation,
        intent,
        pane_anchor,
    )
}

/// State-locked form of [`plan_explicit_name_move`] for a caller that already
/// holds the project-state lock. It never persists anything and never scans a
/// journal.
pub(crate) fn plan_explicit_name_move_locked(
    server: &super::Server,
    state: &State,
    worker_id: &str,
    operation_id: &str,
    invocation: &str,
    intent: &NameIntent,
    pane_anchor: Option<super::name_state::NamePaneAnchor>,
) -> Result<Option<PlannedNameMove>, String> {
    use super::name_state::NameBindingReference;

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
        requested_name: Some(intent.requested_name.clone()),
        caller_worker_id: worker_id.into(),
        caller_session_id: session,
        caller_binding: NameBindingReference {
            app_scope_id: binding.app_scope_id.to_string(),
            binding_id: binding.binding_id.to_string(),
            endpoint_generation: binding.endpoint_generation,
        },
        replace_confirmed: intent.replace_confirmed,
        requested_ms: super::now_ms(),
    };
    let is_master =
        super::current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker_id);
    let input = NameResolutionInput {
        project_scope: route.project_scope_id.as_str().to_owned(),
        operation_id: operation_id.into(),
        request,
        intent: Some(intent.clone()),
        is_current_master: is_master,
        pane_anchor,
    };
    let plan = plan_name_resolution(&state, &input)?;
    match plan.decision {
        NameResolutionDecision::Committed { occupancy, .. } => {
            if occupancy.name == "Jenny" {
                return Ok(None);
            }
            let source_names = names_held_by_worker(&state, &input.project_scope, worker_id);
            if source_names.len() == 1 && source_names[0] == occupancy.name {
                return Ok(None);
            }
            Ok(Some(PlannedNameMove {
                target: occupancy.name,
                source_names,
            }))
        }
        NameResolutionDecision::Pending { .. } => Ok(None),
    }
}

/// Read-only planning for the automatic (non-explicit) name decision. It
/// returns only the committed target name; unlike an explicit rename it never
/// retires the carrier's other names. `None` means the planner produced a
/// pending decision. An automatic decision cannot normally pend, but the
/// caller treats it as "no control to commit" instead of guessing a name.
pub(crate) fn plan_automatic_name(
    server: &super::Server,
    worker_id: &str,
    operation_id: &str,
    invocation: &str,
    pane_anchor: Option<super::name_state::NamePaneAnchor>,
) -> Result<Option<String>, String> {
    let state = server.state.lock().unwrap();
    plan_automatic_name_locked(
        server,
        &state,
        worker_id,
        operation_id,
        invocation,
        pane_anchor,
    )
}

/// State-locked form of [`plan_automatic_name`] for a caller that already
/// holds the project-state lock. It never persists anything and never scans a
/// journal.
pub(crate) fn plan_automatic_name_locked(
    server: &super::Server,
    state: &State,
    worker_id: &str,
    operation_id: &str,
    invocation: &str,
    pane_anchor: Option<super::name_state::NamePaneAnchor>,
) -> Result<Option<String>, String> {
    use super::name_state::NameBindingReference;

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
        requested_name: None,
        caller_worker_id: worker_id.into(),
        caller_session_id: session,
        caller_binding: NameBindingReference {
            app_scope_id: binding.app_scope_id.to_string(),
            binding_id: binding.binding_id.to_string(),
            endpoint_generation: binding.endpoint_generation,
        },
        replace_confirmed: false,
        requested_ms: super::now_ms(),
    };
    let is_master =
        super::current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker_id);
    let input = NameResolutionInput {
        project_scope: route.project_scope_id.as_str().to_owned(),
        operation_id: operation_id.into(),
        request,
        intent: None,
        is_current_master: is_master,
        pane_anchor,
    };
    let plan = plan_name_resolution(&state, &input)?;
    match plan.decision {
        NameResolutionDecision::Committed { occupancy, .. } => Ok(Some(occupancy.name)),
        NameResolutionDecision::Pending { .. } => Ok(None),
    }
}

/// Pure automatic-name planning for a Register candidate that has not been
/// consumed yet. The prepared binding and transport are the only current
/// facts needed; this deliberately does not require the binding to already be
/// present in the reducer, so the control slot can be published before the
/// business Register commit.
pub(crate) fn plan_automatic_name_for_binding(
    state: &State,
    operation_id: &str,
    invocation: &str,
    binding: &super::RuntimeBinding,
    pane_anchor: Option<super::name_state::NamePaneAnchor>,
) -> Result<Option<String>, String> {
    if binding.agent_id.as_str().trim().is_empty() {
        return Err("NAME_IDENTITY_REQUIRED: candidate binding has no worker".into());
    }
    let session = binding
        .session_id
        .as_ref()
        .ok_or("NAME_IDENTITY_REQUIRED: candidate binding has no agent session")?
        .to_string();
    let request = NameRecoveryRequest {
        invocation: invocation.into(),
        requested_name: None,
        caller_worker_id: binding.agent_id.as_str().to_owned(),
        caller_session_id: session,
        caller_binding: super::name_state::NameBindingReference {
            app_scope_id: binding.app_scope_id.to_string(),
            binding_id: binding.binding_id.to_string(),
            endpoint_generation: binding.endpoint_generation,
        },
        replace_confirmed: false,
        requested_ms: super::now_ms(),
    };
    let route = binding.route_scope();
    let is_master =
        super::current_master_worker_id(state, Some(&route)).as_deref() == Some(binding.agent_id.as_str());
    let input = NameResolutionInput {
        project_scope: binding.project_scope.as_str().to_owned(),
        operation_id: operation_id.into(),
        request,
        intent: None,
        is_current_master: is_master,
        pane_anchor,
    };
    let plan = plan_name_resolution(state, &input)?;
    match plan.decision {
        NameResolutionDecision::Committed { occupancy, .. } => Ok(Some(occupancy.name)),
        NameResolutionDecision::Pending { .. } => Ok(None),
    }
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
        state
            .name_occupancy(project, name)
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

/// Names currently carried by one worker. Only an explicit rename (or an
/// explicit authorized name change) uses these as handoff sources; automatic
/// session or pane inheritance must never treat the same worker id as an
/// identity anchor.
pub(crate) fn names_held_by_worker(
    state: &State,
    project_scope: &str,
    worker_id: &str,
) -> Vec<String> {
    let mut names: Vec<String> = state
        .name_occupancies
        .get(project_scope)
        .into_iter()
        .flat_map(|names| names.values())
        .filter(|holder| holder.worker_id == worker_id)
        .map(|holder| holder.name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Call with the current locked State and an admitted stable worker ID. No
/// transport, historical label, activity, or old-session gate decides ownership.
fn match_pane_holder<'a>(
    names: &'a std::collections::HashMap<String, NameOccupancy>,
    anchor: &super::name_state::NamePaneAnchor,
) -> Result<Option<&'a NameOccupancy>, String> {
    let mut matches = names.values().filter(|holder| {
        holder.pane_anchor.as_ref().is_some_and(|stored| {
            stored.pane.socket_path == anchor.pane.socket_path
                && stored.pane.pane_id == anchor.pane.pane_id
                && stored.cwd == anchor.cwd
        })
    });
    let first = matches.next();
    let second = matches.next();
    match (first, second) {
        (Some(holder), None) => Ok(Some(holder)),
        (Some(_), Some(_)) => {
            Err("NAME_PANE_CONFLICT: the current pane and cwd match multiple names".into())
        }
        (None, _) => Ok(None),
    }
}

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
    if input.intent.is_none() && (request.requested_name.is_some() || request.replace_confirmed) {
        return Err("NAME_AUTHORIZATION_REQUIRED: requested name needs typed name intent".into());
    }
    let names = state.name_occupancies.get(&input.project_scope);
    let target = if let Some(intent) = &input.intent {
        // An explicit name is decided before any pane lookup. A pane conflict
        // must not block an authorized explicit rename.
        intent.requested_name.clone()
    } else {
        // Same sessionID inherits the current name, including Jenny. A name is
        // not a master grant; the grant stays on its typed owner.
        let same_session = if let Some(names) = names {
            let mut matches = names
                .values()
                .filter(|holder| holder.session_id == request.caller_session_id);
            let first = matches.next();
            if matches.next().is_some() {
                return Err("NAME_SESSION_CONFLICT: the current session matches multiple names".into());
            }
            first
        } else {
            None
        };
        if let Some(holder) = same_session {
            holder.name.clone()
        } else {
            // Only after explicit and same-session anchors are absent does the
            // exact pane endpoint plus invocation cwd participate. The lookup
            // stays in this function so a conflict can be returned directly.
            let pane_holder = match (input.pane_anchor.as_ref(), names) {
                (Some(anchor), Some(names)) => match_pane_holder(names, anchor)?,
                _ => None,
            };
            let worker_holder = names
                .map(|names| {
                    names
                        .values()
                        .filter(|holder| holder.worker_id == request.caller_worker_id)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if let Some(holder) = pane_holder {
                holder.name.clone()
            } else if worker_holder.len() > 1 {
                return Err(
                    "NAME_WORKER_CONFLICT: the current worker matches multiple names".into(),
                );
            } else if let Some(holder) = worker_holder.first() {
                // A session change on the same stable worker id keeps the
                // worker's existing name when no pane anchor identifies a
                // specific placement. Ownership follows the current worker,
                // not the retired session.
                holder.name.clone()
            } else if input.is_current_master {
                "Jenny".into()
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
            }
        }
    };
    // Only an explicit Jenny intent is a master transfer. Automatic name
    // inheritance, including a pane match on Jenny, never copies the grant.
    let explicit_jenny = input
        .intent
        .as_ref()
        .is_some_and(|intent| intent.requested_name == "Jenny");
    let requires_master_authorization = explicit_jenny && !input.is_current_master;
    let previous = state.name_occupancy(&input.project_scope, &target);
    // A legacy promote approval is free text: it authorizes an uncontested
    // first Jenny, but it must not dislodge a Jenny held by another carrier.
    // That continuation needs a typed restore_master with replace_confirmed.
    if let Some(holder) = previous {
        let other_carrier = holder.worker_id != request.caller_worker_id
            || holder.session_id != request.caller_session_id;
        if other_carrier
            && explicit_jenny
            && !input
                .intent
                .as_ref()
                .is_some_and(|intent| intent.replace_confirmed)
        {
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
            let events = vec![Event::NameRecoveryUpdated {
                recovery: recovery.clone(),
            }];
            return Ok(NameResolutionPlan {
                decision: NameResolutionDecision::Pending { recovery },
                events,
                requires_master_authorization,
            });
        }
    }
    let occupancy = match previous {
        Some(holder)
            if holder.worker_id == request.caller_worker_id
                && holder.session_id == request.caller_session_id =>
        {
            let mut holder = holder.clone();
            holder.pane_anchor = input.pane_anchor.clone();
            holder
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
            pane_anchor: input.pane_anchor.clone(),
        },
    };
    let rename_from = if input
        .intent
        .as_ref()
        .is_some_and(|intent| intent.moves_existing_names)
    {
        names_held_by_worker(state, &input.project_scope, &request.caller_worker_id)
    } else {
        Vec::new()
    };
    let events = build_name_resolution_events_with_rename_from(
        state,
        &input.project_scope,
        &occupancy,
        &rename_from,
    );
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
    build_name_resolution_events_with_rename_from(state, project_scope, occupancy, &[])
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
) -> Vec<Event> {
    let previous = state.name_occupancy(project_scope, &occupancy.name);
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
        let matches_target_name = existing
            .is_some_and(|assignment| assignment.name == occupancy.name && target_held_by_other);
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
        rename_from,
    ));
    // The name carries its finite goal schedules to the new execution carrier.
    // Reuse the subscription record/event so IDs, timing and terminal state stay
    // intact, and the normal worker authorization still rejects the old caller.
    let previous_changed = previous.as_ref().is_some_and(|previous| {
        previous.worker_id != occupancy.worker_id || previous.session_id != occupancy.session_id
    });
    let mut handoff_worker_ids: Vec<String> = state
        .name_occupancies
        .get(project_scope)
        .into_iter()
        .flat_map(|names| names.values())
        .filter(|holder| {
            holder.session_id == occupancy.session_id && holder.worker_id != occupancy.worker_id
        })
        .map(|holder| holder.worker_id.clone())
        .collect();
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
                    subscription.target = target.into();
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
