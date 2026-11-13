fn work_executor_gate(
    server: &Server,
    state: &State,
    task_id: &str,
    actor: &str,
) -> Result<(), Resp> {
    let route = server_route_scope(server, state)
        .map_err(Resp::err)?
        .ok_or_else(|| Resp::err("WORK_EXECUTOR_PROJECT_REQUIRED"))?;
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .map_err(Resp::err)?;
    work_executor_gate_with_control(state, &control, &route, task_id, actor)
}

fn work_executor_gate_with_control(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    task_id: &str,
    actor: &str,
) -> Result<(), Resp> {
    if let Some(name) = state.effective_work_policy(task_id).executor_name {
        let matches = control
            .current_for_name(&route.project_scope_id, &route.app_scope_id, &name)
            .map_err(Resp::err)?
            .and_then(|slot| slot.active_binding())
            .is_some_and(|current| current.binding.agent_id.as_str() == actor);
        if !matches {
            return Err(Resp::err_data(
                "WORK_EXECUTOR_REQUIRED: task is pinned to another stable name",
                json!({"task_id":task_id,"executor_name":name,"delivery_complete":false}),
            ));
        }
    }
    Ok(())
}

fn task_owner_gate(
    server: &Server,
    state: &State,
    task_id: &str,
    actor: &str,
) -> Result<(), Resp> {
    task_owner_gate_maybe_with_control(server, state, None, task_id, actor)
}

fn task_owner_gate_maybe_with_control(
    server: &Server,
    state: &State,
    control: Option<&binding_slot::BindingControlSet>,
    task_id: &str,
    actor: &str,
) -> Result<(), Resp> {
    if state.task_assignment(task_id).is_none() {
        return legacy_task_owner_gate(state, task_id, actor);
    }
    let route = server_route_scope(server, state)
        .map_err(Resp::err)?
        .ok_or_else(|| Resp::err("TASK_OWNER_PROJECT_REQUIRED"))?;
    let loaded;
    let control = match control {
        Some(control) => control,
        None => {
            loaded = binding_store::BindingControlStore::new(server.host_paths.state_root())
                .load()
                .map_err(Resp::err)?;
            &loaded
        }
    };
    task_owner_gate_with_control(state, control, &route, task_id, actor)
}

fn task_owner_gate_with_control(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    task_id: &str,
    actor: &str,
) -> Result<(), Resp> {
    if !state.tasks.contains_key(task_id) {
        return Err(Resp::err(format!("task {task_id} not found")));
    }
    if let Some(assignment) = state.task_assignment(task_id) {
        let matches = control
            .current_for_name(
                &route.project_scope_id,
                &route.app_scope_id,
                &assignment.name,
            )
            .map_err(Resp::err)?
            .and_then(|slot| slot.active_binding())
            .is_some_and(|current| current.binding.agent_id.as_str() == actor);
        if !matches {
            return Err(Resp::err_data(
                "TASK_OWNER_REQUIRED: the task's current stable-name holder may execute it",
                json!({
                    "task_id": task_id,
                    "assignment_name": assignment.name,
                    "delivery_complete": false,
                }),
            ));
        }
        return Ok(());
    }
    legacy_task_owner_gate(state, task_id, actor)
}

fn legacy_task_owner_gate(state: &State, task_id: &str, actor: &str) -> Result<(), Resp> {
    let Some(task) = state.tasks.get(task_id) else {
        return Err(Resp::err(format!("task {task_id} not found")));
    };
    if task.owner != actor {
        return Err(Resp::err_data(
            "TASK_OWNER_REQUIRED: only the legacy task owner may execute it",
            json!({
                "task_id": task_id,
                "legacy_owner": task.owner,
                "delivery_complete": false,
            }),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutionCandidateIneligible {
    PinnedExecutorMismatch,
    RequiredReviewer,
}

fn execution_candidate_eligibility(
    policy: &crate::board::WorkPolicy,
    candidate_name: &str,
) -> Result<(), ExecutionCandidateIneligible> {
    if policy
        .executor_name
        .as_deref()
        .is_some_and(|executor| executor != candidate_name)
    {
        return Err(ExecutionCandidateIneligible::PinnedExecutorMismatch);
    }
    if policy.reviewer_name.as_deref() == Some(candidate_name) {
        return Err(ExecutionCandidateIneligible::RequiredReviewer);
    }
    Ok(())
}

fn execution_candidate_gate(
    state: &State,
    task_id: &str,
    candidate_name: &str,
) -> Result<(), Resp> {
    let policy = state.effective_work_policy(task_id);
    match execution_candidate_eligibility(&policy, candidate_name) {
        Ok(()) => Ok(()),
        Err(ExecutionCandidateIneligible::PinnedExecutorMismatch) => Err(Resp::err_data(
            "WORK_EXECUTOR_REQUIRED: task is pinned to another stable name",
            json!({
                "task_id": task_id,
                "candidate_name": candidate_name,
                "executor_name": policy.executor_name,
                "delivery_complete": false
            }),
        )),
        Err(ExecutionCandidateIneligible::RequiredReviewer) => Err(Resp::err_data(
            "WORK_REVIEWER_EXECUTION_CONFLICT: required reviewer cannot execute the same task",
            json!({
                "task_id": task_id,
                "candidate_name": candidate_name,
                "reviewer_name": policy.reviewer_name,
                "delivery_complete": false
            }),
        )),
    }
}

fn execution_admission_gate(
    server: &Server,
    state: &State,
    task_id: &str,
    actor: &str,
    candidate_name: &str,
) -> Result<(), Resp> {
    work_executor_gate(server, state, task_id, actor)?;
    execution_candidate_gate(state, task_id, candidate_name)
}

fn execution_admission_gate_with_control(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    task_id: &str,
    actor: &str,
    candidate_name: &str,
) -> Result<(), Resp> {
    work_executor_gate_with_control(state, control, route, task_id, actor)?;
    execution_candidate_gate(state, task_id, candidate_name)
}

fn execution_admission_gate_for_actor(
    server: &Server,
    state: &State,
    task_id: &str,
    actor: &str,
) -> Result<(), Resp> {
    let route = server_route_scope(server, state)
        .map_err(Resp::err)?
        .ok_or_else(|| Resp::err("WORK_EXECUTOR_PROJECT_REQUIRED"))?;
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .map_err(Resp::err)?;
    let Some(slot) = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, actor)
        .map_err(Resp::err)?
    else {
        return Ok(());
    };
    execution_admission_gate_with_control(
        state,
        &control,
        &route,
        task_id,
        actor,
        slot.identity_key.name.as_str(),
    )
}

fn parse_work_policy(
    kind: Option<String>,
    prefer_role: Option<String>,
    executor: Option<String>,
    reviewer: Option<String>,
    clear_roles: bool,
    clear_executor: bool,
    clear_reviewer: bool,
) -> Result<crate::board::WorkPolicy, Resp> {
    fn name(value: Option<String>) -> Result<Option<String>, Resp> {
        value.map(|value| {
            let value=value.trim();
            if value.is_empty() || value.chars().any(char::is_control) {
                Err(Resp::err("WORK_POLICY_INVALID: values must be nonempty and contain no control characters"))
            } else { Ok(value.to_owned()) }
        }).transpose()
    }
    if (clear_roles && prefer_role.is_some())
        || (clear_executor && executor.is_some())
        || (clear_reviewer && reviewer.is_some())
    {
        return Err(Resp::err(
            "WORK_POLICY_INVALID: choose a value or clear, not both",
        ));
    }
    let preferred_roles = prefer_role
        .map(|text| {
            let mut values = Vec::new();
            for value in text.split(',') {
                values.push(name(Some(value.to_lowercase()))?.unwrap());
            }
            values.sort();
            values.dedup();
            Ok::<_, Resp>(values)
        })
        .transpose()?;
    let mut cleared_fields = Vec::new();
    if clear_roles {
        cleared_fields.push("preferred_roles".into());
    }
    if clear_executor {
        cleared_fields.push("executor_name".into());
    }
    if clear_reviewer {
        cleared_fields.push("reviewer_name".into());
    }
    Ok(crate::board::WorkPolicy {
        work_kind: name(kind)?,
        preferred_roles,
        executor_name: name(executor)?,
        reviewer_name: name(reviewer)?,
        cleared_fields,
    })
}

fn overlay_work_policy(base: &mut crate::board::WorkPolicy, patch: &crate::board::WorkPolicy) {
    let cleared = |field: &str| patch.cleared_fields.iter().any(|value| value == field);
    crate::board::apply_policy_field(&patch.work_kind, cleared("work_kind"), &mut base.work_kind);
    crate::board::apply_policy_field(
        &patch.preferred_roles,
        cleared("preferred_roles"),
        &mut base.preferred_roles,
    );
    crate::board::apply_policy_field(
        &patch.executor_name,
        cleared("executor_name"),
        &mut base.executor_name,
    );
    crate::board::apply_policy_field(
        &patch.reviewer_name,
        cleared("reviewer_name"),
        &mut base.reviewer_name,
    );
    for field in [
        "work_kind",
        "preferred_roles",
        "executor_name",
        "reviewer_name",
    ] {
        let supplied = match field {
            "work_kind" => patch.work_kind.is_some(),
            "preferred_roles" => patch.preferred_roles.is_some(),
            "executor_name" => patch.executor_name.is_some(),
            _ => patch.reviewer_name.is_some(),
        };
        if supplied {
            base.cleared_fields.retain(|entry| entry != field);
        }
        if cleared(field) && !base.cleared_fields.iter().any(|entry| entry == field) {
            base.cleared_fields.push(field.into());
        }
    }
}

/// Stable-name review capability used by the three-tier review route.
pub(crate) fn ticket_review_group(
    state: &State,
    route: &RouteScope,
    slot_name: &str,
) -> Option<crate::server::ticket_state::ReviewRouteGroup> {
    use crate::server::ticket_state::ReviewRouteGroup;
    let roles = state.name_work_roles(route.project_scope_id.as_str(), slot_name);
    if roles.iter().any(|role| role == "reviewer") {
        return Some(ReviewRouteGroup::Reviewer);
    }
    if roles
        .iter()
        .any(|role| role == "architect" || role == "expert")
    {
        return Some(ReviewRouteGroup::ArchitectExpert);
    }
    Some(ReviewRouteGroup::IndependentPeer)
}

/// Phase-group queue key. Execute and plan use the work-kind group already
/// persisted for rotation; reviews use one group per review subject.
pub(crate) fn ticket_phase_group(
    phase: crate::server::ticket_state::TicketPhaseKind,
    work_kind: Option<&str>,
) -> String {
    use crate::server::ticket_state::TicketPhaseKind;
    let kind = work_kind.unwrap_or("generic");
    match phase {
        TicketPhaseKind::Execute => format!("execute:{kind}"),
        TicketPhaseKind::Plan => format!("plan:{kind}"),
        TicketPhaseKind::PlanReview => "plan_review".into(),
        TicketPhaseKind::FinalReview => "final_review".into(),
    }
}

/// Revoke execute attempts whose binding generation was replaced by the
/// current stable-name carrier and queue one fresh attempt batch for each
/// affected task. The caller must already hold the register gate and the
/// runtime state lock, and must commit the returned events with the same
/// business transaction.
pub(crate) fn ticket_binding_recovery_events(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    slot_name: &str,
) -> Result<Vec<Event>, Resp> {
    use crate::server::ticket_state::{
        stale_binding_execute_phases, TicketPhaseKind, TicketPhaseStatus,
    };

    let Some(current_slot) = control
        .current_for_name(
            &route.project_scope_id,
            &route.app_scope_id,
            slot_name,
        )
        .map_err(Resp::err)?
    else {
        return Ok(Vec::new());
    };
    let Some(current) = current_slot.active_binding() else {
        return Ok(Vec::new());
    };
    let current_worker_id = current.binding.agent_id.as_str();
    let Some(current_session) = current.binding.session_id.as_ref() else {
        return Ok(Vec::new());
    };
    let current_generation = current_slot.generation;
    let stale = stale_binding_execute_phases(
        state.ticket_phases.iter(),
        slot_name,
        current_generation,
    );

    let mut events = Vec::new();
    let mut recovered_tasks = std::collections::BTreeSet::new();
    for phase in stale {
        if !recovered_tasks.insert(phase.task_id.clone()) {
            continue;
        }
        let Some(task) = state.tasks.get(&phase.task_id) else {
            continue;
        };
        if matches!(task.status.as_str(), "closed" | "cancelled") {
            continue;
        }
        if task.status != "working" || task.owner != current_worker_id {
            // The approved recovery model keeps accepted responsibility with
            // the current stable-name carrier.
            continue;
        }
        let Some(assignment) = state.task_assignment(&phase.task_id) else {
            continue;
        };
        if assignment.name != slot_name || assignment.session_id != current_session.as_str() {
            continue;
        }
        // A newer execute batch already owns this task. This also makes replay
        // of the same handoff effect idempotent after the first recovery commit.
        if state.ticket_phases.iter().any(|candidate| {
            candidate.task_id == phase.task_id
                && candidate.phase == TicketPhaseKind::Execute
                && candidate.batch > phase.batch
        }) {
            continue;
        }
        let Some(next_batch) = phase.batch.checked_add(1) else {
            return Err(Resp::err_data(
                "TICKET_BATCH_EXHAUSTED: cannot allocate a recovery batch",
                json!({"task_id": phase.task_id, "batch": phase.batch}),
            ));
        };

        let mut revoked = phase.clone();
        revoked.status = TicketPhaseStatus::Revoked;

        let mut queued = phase.clone();
        queued.batch = next_batch;
        queued.status = TicketPhaseStatus::Queued;
        queued.binding_generation = 0;
        queued.result = None;
        queued.wait_reason = None;

        events.push(Event::TicketPhaseRecorded { phase: revoked });
        events.push(Event::TicketPhaseRecorded { phase: queued });
    }
    Ok(events)
}

/// Exact eligibility for a recovered re-claim. A task may be re-claimed only
/// when it was already accepted by the current stable name, its assignment
/// still points at the current session, an older claimed execute attempt was
/// revoked, and the newest execute batch is still queued with no overlapping
/// active attempt. This is shared by the manual claim and continuation paths.
pub(crate) fn ticket_recovery_claim_phase(
    state: &State,
    slot_name: &str,
    current_session: &str,
    task_id: &str,
    current_worker_id: &str,
    current_generation: u64,
) -> Option<crate::server::ticket_state::TicketPhaseRecord> {
    use crate::server::ticket_state::{TicketPhaseKind, TicketPhaseStatus};

    if !state
        .board_details
        .get(task_id)
        .is_some_and(|details| details.admitted)
    {
        return None;
    }
    let task = state.tasks.get(task_id)?;
    if task.status != "working" || task.owner != current_worker_id {
        return None;
    }
    let assignment = state.task_assignment(task_id)?;
    if assignment.name != slot_name || assignment.session_id != current_session {
        return None;
    }
    let mut execute: Vec<_> = state
        .ticket_phases
        .iter()
        .filter(|phase| {
            phase.task_id == task_id
                && phase.phase == TicketPhaseKind::Execute
                && phase.slot_name.as_deref() == Some(slot_name)
        })
        .collect();
    execute.sort_by_key(|phase| phase.batch);
    if execute.iter().any(|phase| {
        matches!(
            phase.status,
            TicketPhaseStatus::Claimed | TicketPhaseStatus::Running
        )
    }) {
        return None;
    }
    let queued = execute.last()?;
    if queued.status != TicketPhaseStatus::Queued {
        return None;
    }
    let has_old_claim = execute.iter().any(|phase| {
        phase.batch < queued.batch
            && phase.binding_generation > 0
            && phase.binding_generation < current_generation
            && phase.attempt_generation == queued.attempt_generation
            && phase.status == TicketPhaseStatus::Revoked
    });
    if !has_old_claim {
        return None;
    }
    Some((*queued).clone())
}

/// Release an in-flight stale attempt left behind by a daemon that had no
/// bind-time recovery. Unlike `ticket_recovery_claim_phase`, this needs no
/// recovery batch because no pending control effect exists to have queued one.
/// The current stable-name carrier may still release the slot through the
/// public claim path when the newest execute attempt is Claimed/Running at an
/// older generation, or when assignment and task ownership already moved to
/// this slot while the newest execute attempt is still on another slot with no
/// submitted result. `rework` is included because a reopened rework attempt
/// can be left at an older generation with no other recovery path.
pub(crate) fn ticket_stale_active_claim_phase(
    state: &State,
    slot_name: &str,
    current_session: &str,
    task_id: &str,
    current_worker_id: &str,
    current_generation: u64,
) -> Option<crate::server::ticket_state::TicketPhaseRecord> {
    use crate::server::ticket_state::{TicketPhaseKind, TicketPhaseStatus};

    if !state
        .board_details
        .get(task_id)
        .is_some_and(|details| details.admitted)
    {
        return None;
    }
    let task = state.tasks.get(task_id)?;
    if !matches!(task.status.as_str(), "working" | "blocked" | "rework")
        || task.owner != current_worker_id
    {
        return None;
    }
    let assignment = state.task_assignment(task_id)?;
    if assignment.name != slot_name || assignment.session_id != current_session {
        return None;
    }
    let mut execute: Vec<_> = state
        .ticket_phases
        .iter()
        .filter(|phase| {
            phase.task_id == task_id
                && phase.phase == TicketPhaseKind::Execute
        })
        .collect();
    execute.sort_by_key(|phase| phase.batch);
    let newest = execute.last()?;
    if !matches!(
        newest.status,
        TicketPhaseStatus::Claimed | TicketPhaseStatus::Running
    ) {
        return None;
    }
    let same_slot = newest.slot_name.as_deref() == Some(slot_name);
    let stale_same_slot =
        same_slot && newest.binding_generation > 0 && newest.binding_generation < current_generation;
    let stale_cross_slot = !same_slot
        && newest.slot_name.is_some()
        && newest.binding_generation > 0
        && newest.result.is_none();
    if !stale_same_slot && !stale_cross_slot {
        return None;
    }
    Some((*newest).clone())
}

/// Eligible review receivers for one admitted ticket phase. A participating
/// actor or stable name is excluded by the route itself; the caller only has to
/// supply every bound, project-authorized peer.
pub(crate) fn ticket_review_candidates(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    phases: &[crate::server::ticket_state::TicketPhaseRecord],
) -> Vec<crate::server::ticket_state::ReviewCandidate> {
    use crate::server::ticket_state::{ReviewCandidate, ReviewRouteGroup};
    let master = current_master_worker_id(state, None);
    let mut candidates = Vec::new();
    for worker in state.workers.values() {
        if is_managed_subagent(state, &worker.id)
            || authoritative_send_binding(state, route, &worker.id).is_err()
        {
            continue;
        }
        if master.as_deref() == Some(worker.id.as_str()) {
            continue;
        }
        let Ok(Some(slot)) = control.current_for_worker(
            &route.project_scope_id,
            &route.app_scope_id,
            &worker.id,
        ) else {
            continue;
        };
        let name = slot.identity_key.name.clone();
        let Some(group) = ticket_review_group(state, route, &name) else {
            continue;
        };
        let participated = phases.iter().any(|phase| {
            phase.slot_name.as_deref() == Some(name.as_str())
                && phase
                    .result
                    .as_ref()
                    .is_some_and(|result| result.submitted_by == worker.id)
        }) || phases.iter().any(|phase| {
            phase.slot_name.as_deref() == Some(name.as_str())
                && matches!(
                    phase.status,
                    crate::server::ticket_state::TicketPhaseStatus::Claimed
                        | crate::server::ticket_state::TicketPhaseStatus::Running
                )
        });
        candidates.push(ReviewCandidate {
            slot_name: name,
            worker_id: worker.id.clone(),
            group,
            participated,
        });
    }
    candidates.sort_by(|left, right| left.slot_name.cmp(&right.slot_name));
    candidates.dedup_by(|left, right| left.slot_name == right.slot_name);
    candidates
}

/// Eligible non-master slots for a plan or execute phase. Plan requires a
/// `plan` or `planner` specialty. Execute is the project default for every
/// valid peer. A specified executor pins only execute, never the plan phase.
fn ticket_slot_candidates(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    policy: &crate::board::WorkPolicy,
    phase: crate::server::ticket_state::TicketPhaseKind,
) -> Result<Vec<crate::server::ticket_state::TicketCandidate>, Resp> {
    use crate::server::ticket_state::{TicketCandidate, TicketPhaseKind};
    let available = |worker: &WorkerRec| {
        !is_managed_subagent(state, &worker.id)
            && authoritative_send_binding(state, route, &worker.id).is_ok()
            && !state
                .agent_presence(route.project_scope_id.as_str(), &worker.id)
                .is_some_and(|presence| presence.presence == name_state::AgentPresence::Offline)
    };
    let mut candidates = Vec::new();
    if phase == TicketPhaseKind::Execute {
        if let Some(name) = policy.executor_name.as_deref() {
            if execution_candidate_eligibility(policy, name).is_err() {
                return Ok(candidates);
            }
            let current = control
                .current_for_name(&route.project_scope_id, &route.app_scope_id, name)
                .map_err(Resp::err)?;
            if let Some(worker) = current
                .and_then(|slot| slot.active_binding())
                .and_then(|current| state.workers.get(current.binding.agent_id.as_str()))
            {
                if available(worker) {
                    candidates.push(TicketCandidate {
                        slot_name: name.to_owned(),
                        worker_id: worker.id.clone(),
                        matched_preferred_role: true,
                    });
                }
            }
            return Ok(candidates);
        }
    }
    let master = current_master_worker_id(state, None);
    for worker in state.workers.values().filter(|worker| available(worker)) {
        if master.as_deref() == Some(worker.id.as_str()) {
            continue;
        }
        let slot = control
            .current_for_worker(&route.project_scope_id, &route.app_scope_id, &worker.id)
            .map_err(Resp::err)?;
        let Some(slot) = slot else { continue };
        let name = slot.identity_key.name.clone();
        if phase == TicketPhaseKind::Execute
            && execution_candidate_eligibility(policy, &name).is_err()
        {
            continue;
        }
        let roles = state.name_work_roles(route.project_scope_id.as_str(), &name);
        if phase == TicketPhaseKind::Plan
            && !roles.iter().any(|role| role == "plan" || role == "planner")
        {
            continue;
        }
        let matched = policy
            .preferred_roles
            .as_ref()
            .is_some_and(|preferred| preferred.iter().any(|role| roles.contains(role)));
        candidates.push(TicketCandidate {
            slot_name: name,
            worker_id: worker.id.clone(),
            matched_preferred_role: matched,
        });
    }
    if phase == TicketPhaseKind::Execute {
        if let Some(preferred) = policy.preferred_roles.as_ref() {
            if !preferred.is_empty() {
                candidates.retain(|candidate| candidate.matched_preferred_role);
            }
        }
    }
    candidates.sort_by(|left, right| left.slot_name.cmp(&right.slot_name));
    candidates.dedup_by(|left, right| left.slot_name == right.slot_name);
    Ok(candidates)
}

fn board_work_policy(
    server: &Server,
    actor: String,
    token: String,
    scope: String,
    id: Option<String>,
    patch: crate::board::WorkPolicy,
    expected_revision: Option<u64>,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    board_work_policy_locked(
        server,
        &mut state,
        None,
        actor,
        token,
        scope,
        id,
        patch,
        expected_revision,
    )
}

#[allow(clippy::too_many_arguments)]
fn board_work_policy_locked(
    server: &Server,
    state: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
    actor: String,
    token: String,
    scope: String,
    id: Option<String>,
    patch: crate::board::WorkPolicy,
    expected_revision: Option<u64>,
) -> Resp {
    if let Err(error) = board_check_actor(&state, &actor, &token) {
        return error;
    }
    if scope == "project" {
        if let Err(error) = board_check_master_locked(server, &state, &actor) {
            return error;
        }
        let Some(kind) = patch.work_kind.as_ref() else {
            return Resp::err("project policy requires --work-kind");
        };
        if id.is_some() || expected_revision.is_some() {
            return Resp::err("project policy does not select a task or revision");
        }
        let mut policy = state
            .project_work_policy
            .get(kind)
            .cloned()
            .unwrap_or_default();
        overlay_work_policy(&mut policy, &patch);
        let events = [Event::ProjectWorkPolicyUpdated {
            work_kind: kind.clone(),
            policy: policy.clone(),
        }];
        let commit = match control {
            Some(control) => server
                .commit_locked_checked_with_control(state, &events, control)
                .map(|_| ())
                .map_err(|error| error.to_string()),
            None => server
                .commit_locked_checked(state, &events)
                .map(|_| ())
                .map_err(|error| error.to_string()),
        };
        if let Err(error) = commit {
            return Resp::err(format!("WORK_POLICY_DURABILITY_FAILED: {error}"));
        }
        return Resp::data(
            json!({"outcome":"rules_updated","scope":"project","work_kind":kind,"policy":policy}),
        );
    }
    if !matches!(scope.as_str(), "goal" | "task") {
        return Resp::err("policy scope must be project, goal or task");
    }
    let Some(id) = id else {
        return Resp::err("goal/task policy requires --id");
    };
    let Some(revision) = expected_revision else {
        return Resp::err("goal/task policy requires --expected-revision");
    };
    let mut details = match board_check_revision(&state, &id, revision) {
        Ok(details) => details,
        Err(error) => return error,
    };
    let Some(mut task) = state.tasks.get(&id).cloned() else {
        return Resp::err("task not found");
    };
    if !board_task_visible(&state, &task, Some(&actor)) {
        return Resp::err("BOARD_PRIVATE_TASK");
    }
    if matches!(task.status.as_str(), "merged" | "closed" | "cancelled") {
        return Resp::err("WORK_POLICY_TERMINAL: create a follow-up task instead");
    }
    if details.invitation.is_some() {
        return Resp::err("withdraw the invitation before changing its contract");
    }
    let mut events = Vec::new();
    if scope == "goal" {
        if let Err(error) = board_check_master_locked(server, &state, &actor) {
            return error;
        }
        let Some(kind) = patch
            .work_kind
            .as_ref()
            .or(details.work_policy.work_kind.as_ref())
            .cloned()
        else {
            return Resp::err("goal policy requires --work-kind");
        };
        let policy = details.child_work_policies.entry(kind).or_default();
        overlay_work_policy(policy, &patch);
    } else {
        if task_owner_gate_maybe_with_control(server, state, control, &id, &actor).is_err() {
            if let Err(error) = board_check_master_locked(server, &state, &actor) {
                return error;
            }
        }
        let previous = details.work_policy.clone();
        overlay_work_policy(&mut details.work_policy, &patch);
        if previous != details.work_policy {
            details.dispatch_selection = None;
            details.dispatch_reason = None;
            // Only a changed review requirement invalidates current acceptance;
            // executor reassignment and selection preferences preserve evidence.
            if previous.reviewer_name != details.work_policy.reviewer_name {
                let mut record = state.task_lifecycle.get(&id).cloned().unwrap_or_default();
                events.extend(invalidate_work_review(
                    &state,
                    &task,
                    &mut record,
                    details.work_policy.reviewer_name.clone(),
                ));
                // A reviewer rebind invalidates the previous verdict, but it is
                // not itself a typed rework decision. Keep a delivered (or
                // accepted) task delivered so the new reviewer can decide, and
                // point next_step at the current required reviewer.
                if matches!(task.status.as_str(), "accepted" | "delivered") {
                    task.status = "delivered".into();
                    task.next_step = Some(match details.work_policy.reviewer_name.as_deref() {
                        Some(name) => format!(
                            "await review by {name}; use collab task review --accept or --rework"
                        ),
                        None => "task owner or live master reviews delivery with collab task review --accept or --rework".into(),
                    });
                }
                if record.delivered_ms.is_some() {
                    events.push(Event::TaskLifecycleUpdated {
                        task_id: id.clone(),
                        record,
                    });
                }
            }
        }
    }
    details.revision += 1;
    task.updated_ms = now_ms();
    let resulting_status = task.status.clone();
    events.extend([
        Event::TaskUpdated { task },
        Event::BoardDetailsChanged {
            task_id: id.clone(),
            details,
        },
    ]);
    // Only an executing state reopens a completed execute phase. A reviewer
    // rebind that keeps the task delivered must not restart execution.
    if matches!(resulting_status.as_str(), "working" | "verifying" | "rework") {
        if let Some(event) = ticket_rework_phase_event(server, &state, &id) {
            events.push(event);
        }
    }
    let commit = match control {
        Some(control) => server
            .commit_locked_checked_with_control(state, &events, control)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        None => server
            .commit_locked_checked(state, &events)
            .map(|_| ())
            .map_err(|error| error.to_string()),
    };
    if let Err(error) = commit {
        return Resp::err(format!("WORK_POLICY_DURABILITY_FAILED: {error}"));
    }
    board_task_response(server, &state, &id, &actor)
}

enum PreparedBoardDispatch {
    Invite(PreparedBoardInvitation),
    Waiting {
        events: Vec<Event>,
        response: serde_json::Value,
    },
}

fn prepare_board_dispatch_locked(
    server: &Server,
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    actor: &str,
    id: &str,
    expected_revision: u64,
    long_horizon: crate::board::LongHorizonArgs,
    now: i64,
) -> Result<PreparedBoardDispatch, Resp> {
    let mut details = board_check_revision(state, id, expected_revision)?;
    details.long_horizon = match long_horizon.merge(details.long_horizon.as_ref()) {
        Ok(Some(spec)) => Some(spec),
        Ok(None) => {
            return Err(Resp::err(
                "BOARD_LONG_HORIZON_CONTENT_REQUIRED: supply long-horizon task content and cadence",
            ))
        }
        Err(error) => return Err(Resp::err(error)),
    };
    let Some(task) = state.tasks.get(id) else {
        return Err(Resp::err("task not found"));
    };
    if !details.public_visibility
        || matches!(task.status.as_str(), "closed" | "cancelled" | "merged")
    {
        return Err(Resp::err(
            "WORK_DISPATCH_UNAVAILABLE: task must be public and open",
        ));
    }
    if details.invitation.is_some() {
        return Err(Resp::err("task already has a pending invitation"));
    }
    let policy = details.work_policy.clone();
    let eligible_ids =
        dispatch_eligible_worker_ids(state, route, control, actor, &policy)?;
    let selected = if let Some(name) = policy.executor_name.as_deref() {
        eligible_ids.first().map(|worker_id| {
            (
                name.to_owned(),
                worker_id.clone(),
                "explicit executor".to_owned(),
            )
        })
    } else {
        let mut candidates = Vec::new();
        for worker_id in &eligible_ids {
            let slot = match control.current_for_worker(
                &route.project_scope_id,
                &route.app_scope_id,
                worker_id,
            ) {
                Ok(Some(slot)) => slot,
                Ok(None) => continue,
                Err(error) => return Err(Resp::err(error)),
            };
            let name = slot.identity_key.name.clone();
            let roles = state.name_work_roles(route.project_scope_id.as_str(), &name);
            let matched = policy
                .preferred_roles
                .as_ref()
                .is_some_and(|preferred| preferred.iter().any(|role| roles.contains(role)));
            let unassigned = !state.tasks.values().any(|task| {
                !matches!(task.status.as_str(), "closed" | "cancelled")
                    && (task.owner == *worker_id
                        || state.board_details.get(&task.id)
                            .and_then(|details| details.invitation.as_ref())
                            .is_some_and(|invite| invite.peer_id == *worker_id))
            });
            candidates.push((matched, unassigned, name, worker_id.clone()));
        }
        candidates.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then(right.1.cmp(&left.1))
                .then(left.2.cmp(&right.2))
        });
        candidates.first().map(|(matched, unassigned, name, id)| {
            let reason = if *matched {
                "matched preferred work role"
            } else if policy
                .preferred_roles
                .as_ref()
                .is_some_and(|roles| !roles.is_empty())
            {
                "no matching work role; selected another available peer"
            } else if *unassigned {
                "selected unassigned peer"
            } else {
                "selected available peer"
            };
            (name.clone(), id.clone(), reason.to_owned())
        })
    };
    let Some((name, peer, reason)) = selected else {
        let reason = policy
            .executor_name
            .as_ref()
            .map(|name| format!("specified executor {name} is unavailable"))
            .unwrap_or_else(|| "no available peer".into());
        details.dispatch_selection = None;
        details.dispatch_reason = Some(reason.clone());
        details.revision += 1;
        return Ok(PreparedBoardDispatch::Waiting {
            events: vec![Event::BoardDetailsChanged {
                task_id: id.to_owned(),
                details,
            }],
            response: json!({
                "outcome": "waiting",
                "task_id": id,
                "reason": reason,
                "delivery_complete": false,
            }),
        });
    };
    let prepared = prepare_board_invitation_locked(
        server,
        state,
        control,
        route,
        Some(actor),
        id,
        &name,
        expected_revision,
        Some((name.clone(), reason)),
        details.long_horizon,
        None,
        now,
    )?;
    let _ = peer;
    Ok(PreparedBoardDispatch::Invite(prepared))
}

fn board_dispatch_work(
    server: &Server,
    actor: String,
    token: String,
    id: String,
    expected_revision: u64,
    long_horizon: crate::board::LongHorizonArgs,
) -> Resp {
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control,
        Err(error) => return Resp::err(error),
    };
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token)
        .and_then(|()| board_check_master_locked(server, &state, &actor))
    {
        return error;
    }
    let route = match server_route_scope(server, &state) {
        Ok(Some(route)) => route,
        _ => return Resp::err("project route is not ready"),
    };
    let prepared = match prepare_board_dispatch_locked(
        server,
        &state,
        &control,
        &route,
        &actor,
        &id,
        expected_revision,
        long_horizon,
        now_ms(),
    ) {
        Ok(prepared) => prepared,
        Err(error) => return error,
    };
    match prepared {
        PreparedBoardDispatch::Waiting { events, response } => {
            if let Err(error) =
                server.commit_locked_checked_with_control(&mut state, &events, &control)
            {
                return Resp::err(format!("WORK_DISPATCH_DURABILITY_FAILED: {error}"));
            }
            Resp::data(response)
        }
        PreparedBoardDispatch::Invite(prepared) => {
            if let Err(error) = server
                .commit_locked_checked_with_control(&mut state, &prepared.events, &control)
            {
                return Resp::err(format!("WORK_DISPATCH_DURABILITY_FAILED: {error}"));
            }
            let master = current_master_worker_id(&state, Some(&route));
            let task = match state
                .tasks
                .get(&id)
                .map(|task| {
                    board_task_view_with_master(
                        &state,
                        &control,
                        task,
                        Some(&actor),
                        master.as_deref(),
                        Some(&route),
                    )
                })
                .transpose()
            {
                Ok(task) => task.flatten(),
                Err(error) => return Resp::err(error),
            };
            let data = json!({
                "task": task,
                "msg_id": prepared.message_id,
                "consumed": false,
                "goal_schedule": task_goal_receipt(&state, &id),
            });
            drop(state);
            let attempt = match &prepared.subscription {
                Some(subscription) => attempt_notification_detailed_with_at(
                    server,
                    &prepared.message_id,
                    &subscription.id,
                    now_ms(),
                ),
                None => NotificationAttempt::NotAttempted(
                    NOTIFICATION_SUBSCRIPTION_MISSING_ERROR.into(),
                ),
            };
            notification_send_response(
                data,
                prepared.subscription.is_some(),
                prepared.method,
                &attempt,
            )
        }
    }
}

fn board_dispatch_current_map(
    manager: &ProjectRuntimeManager,
    server: &Server,
    actor: String,
    token: String,
    id: String,
    expected_revision: u64,
    long_horizon: crate::board::LongHorizonArgs,
) -> Resp {
    let outcome = manager.with_current_state_response(server, |control, state| {
        board_current_map_actor_gate(state, &actor, &token, &id)?;
        board_check_master_locked(server, state, &actor)?;
        let route = server_route_scope(server, state)
            .map_err(Resp::err)?
            .ok_or_else(|| Resp::err("project route is not ready"))?;
        let prepared = prepare_board_dispatch_locked(
            server,
            state,
            control,
            &route,
            &actor,
            &id,
            expected_revision,
            long_horizon,
            now_ms(),
        )?;
        match prepared {
            PreparedBoardDispatch::Waiting { events, response } => {
                server
                    .commit_locked_checked_with_control(state, &events, control)
                    .map_err(|error| {
                        Resp::err(format!("WORK_DISPATCH_DURABILITY_FAILED: {error}"))
                    })?;
                Ok(BoardManagerOutcome::Reply(Resp::data(response)))
            }
            PreparedBoardDispatch::Invite(prepared) => {
                server
                    .commit_locked_checked_with_control(state, &prepared.events, control)
                    .map_err(|error| {
                        Resp::err(format!("WORK_DISPATCH_DURABILITY_FAILED: {error}"))
                    })?;
                let master = current_master_worker_id(state, Some(&route));
                let task = state
                    .tasks
                    .get(&id)
                    .map(|task| {
                        board_task_view_with_master(
                            state,
                            control,
                            task,
                            Some(&actor),
                            master.as_deref(),
                            Some(&route),
                        )
                    })
                    .transpose()
                    .map_err(Resp::err)?
                    .flatten();
                let data = json!({
                    "task": task,
                    "msg_id": prepared.message_id,
                    "consumed": false,
                    "goal_schedule": task_goal_receipt(state, &id),
                });
                Ok(BoardManagerOutcome::Notify(BoardManagerNotification {
                    data,
                    message_id: prepared.message_id,
                    subscription: prepared.subscription,
                    method: prepared.method,
                }))
            }
        }
    });
    match outcome {
        Ok(outcome) => outcome.finish(server),
        Err(response) => response,
    }
}

/// The board dispatch/invite eligibility owner. Reminder observers must call
/// this helper instead of duplicating its stable-name, binding, managed-child
/// or activity reservation rules.
pub(super) fn dispatch_eligible_worker_ids(
    state: &State,
    route: &RouteScope,
    control: &binding_slot::BindingControlSet,
    actor: &str,
    policy: &crate::board::WorkPolicy,
) -> Result<Vec<String>, Resp> {
    let available = |worker: &WorkerRec| {
        worker.id != actor
            && !is_managed_subagent(state, &worker.id)
            && authoritative_send_binding(state, route, &worker.id).is_ok()
            && !state
                .agent_presence(route.project_scope_id.as_str(), &worker.id)
                .is_some_and(|presence| presence.presence == name_state::AgentPresence::Offline)
    };
    if let Some(name) = policy.executor_name.as_deref() {
        if execution_candidate_eligibility(policy, name).is_err() {
            return Ok(Vec::new());
        }
        let current = control
            .current_for_name(&route.project_scope_id, &route.app_scope_id, name)
            .map_err(Resp::err)?;
        return Ok(current
            .and_then(|slot| slot.active_binding())
            .and_then(|current| state.workers.get(current.binding.agent_id.as_str()))
            .filter(|worker| available(worker))
            .map(|worker| worker.id.clone())
            .into_iter()
            .collect());
    }
    let mut eligible = Vec::new();
    for worker in state.workers.values().filter(|worker| available(worker)) {
        let current = control
            .current_for_worker(&route.project_scope_id, &route.app_scope_id, &worker.id)
            .map_err(Resp::err)?;
        if let Some(slot) = current {
            if execution_candidate_eligibility(policy, &slot.identity_key.name).is_err() {
                continue;
            }
            eligible.push(worker.id.clone());
        }
    }
    eligible.sort();
    Ok(eligible)
}

/// Ticket admission: create the first Execute phase for one public task and
/// place it in the shortest eligible slot queue. Returns events for the same
/// commit batch as task creation, so admission is atomic with publish.
pub(crate) fn board_ticket_admission_events(
    server: &Server,
    state: &State,
    task: &TaskRec,
    details: &crate::board::BoardTaskDetails,
    depends_on: Vec<String>,
) -> Result<Vec<Event>, Resp> {
    use crate::server::ticket_state::{
        select_shortest_queue, slot_queue_len, TicketPhaseKind, TicketPhaseRecord,
        TicketPhaseStatus,
    };
    let policy = &details.work_policy;

    let route = match server_route_scope(server, state) {
        Ok(Some(route)) => route,
        _ => return Err(Resp::err("project route is not ready")),
    };
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control,
        Err(error) => return Err(Resp::err(error)),
    };

    let first_phase = TicketPhaseKind::Execute;
    let candidates = ticket_slot_candidates(state, &control, &route, policy, first_phase)?;

    let group = ticket_phase_group(first_phase, policy.work_kind.as_deref());
    let cursor = state.ticket_rotation.get(&group).map(String::as_str);
    let phases = state.ticket_phases.iter().map(|phase| phase).collect::<Vec<_>>();
    let enqueue_seq = state
        .ticket_phases
        .iter()
        .map(|phase| phase.enqueue_seq)
        .max()
        .unwrap_or(0)
        + 1;
    let selected = select_shortest_queue(&candidates, |slot| slot_queue_len(phases.iter().copied(), slot), cursor);

    let mut events = Vec::new();
    let (status, slot_name, wait_reason, selected_worker) = match selected {
        Some(candidate) => {
            events.push(Event::TicketRotationUpdated {
                group: group.clone(),
                slot: candidate.slot_name.clone(),
            });
            (
                TicketPhaseStatus::Queued,
                Some(candidate.slot_name.clone()),
                None,
                Some(candidate.worker_id.clone()),
            )
        }
        None => {
            let reason = if policy.executor_name.is_some() {
                format!(
                    "specified executor {} is unavailable",
                    policy.executor_name.as_deref().unwrap_or_default()
                )
            } else if policy.preferred_roles.as_ref().is_some_and(|roles| !roles.is_empty()) {
                "no peer matches the preferred work roles".to_owned()
            } else {
                "no eligible peer is available".to_owned()
            };
            (TicketPhaseStatus::WaitingAssignment, None, Some(reason), None)
        }
    };
    let phase = TicketPhaseRecord {
        task_id: task.id.clone(),
        phase: first_phase,
        batch: 0,
        priority: task.priority.clone(),
        status,
        slot_name,
        binding_generation: 0,
        enqueue_seq,
        attempt_generation: 0,
        depends_on,
        route_group: None,
        wait_reason,
        result: None,
    };
    events.push(Event::TicketPhaseRecorded { phase: phase.clone() });
    // A publish that lands on an already-idle slot continues in the same
    // transaction. The new phase is offered as a pending overlay because the
    // reducer has not applied it yet.
    if let Some(worker_id) = selected_worker.as_deref() {
        events.extend(ticket_continuation_events_with_pending(
            state,
            &control,
            &route,
            worker_id,
            std::slice::from_ref(&phase),
            Some(task),
            Some(details),
            false,
        ));
    }
    Ok(events)
}

/// The single internal continuation entry for admitted tickets.
///
/// Idle observation, a new enqueue to an already-idle slot, a released result
/// slot and dependency or resource release all run this one planner so the
/// queue rules live in one place. It is a pure function of the current state
/// plus the pending batch overlay; callers own the commit.
///
/// `pending` overlays not-yet-committed phase records onto the state view.
/// `trusted_idle` lets a caller that just observed an online-idle reading assert
/// that fact before the observation is committed.
pub(crate) fn ticket_continuation_events(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    worker_id: &str,
    pending: &[crate::server::ticket_state::TicketPhaseRecord],
    trusted_idle: bool,
) -> Vec<Event> {
    ticket_continuation_events_with_pending(
        state, control, route, worker_id, pending, None, None, trusted_idle,
    )
}

/// Reopen a completed execute phase when the task returns to an executing
/// state. The reopened batch keeps the stable slot when its current binding is
/// active; otherwise it becomes waiting-assignment and the existing matching
/// scope owns the later transition.
pub(crate) fn ticket_rework_phase_event(
    server: &Server,
    state: &State,
    task_id: &str,
) -> Option<Event> {
    use crate::server::ticket_state::{TicketPhaseKind, TicketPhaseStatus};

    if !state
        .board_details
        .get(task_id)
        .is_some_and(|details| details.admitted)
    {
        return None;
    }
    let latest = state
        .ticket_phases
        .iter()
        .filter(|phase| {
            phase.task_id == task_id
                && phase.phase == TicketPhaseKind::Execute
                && phase.status != TicketPhaseStatus::Revoked
        })
        .max_by_key(|phase| phase.batch)
        .cloned()?;
    if !matches!(
        latest.status,
        TicketPhaseStatus::Completed | TicketPhaseStatus::Rework
    ) {
        return None;
    }

    let route = server_route_scope(server, state).ok().flatten()?;
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .ok()?;

    let mut reopened = latest;
    reopened.batch = reopened.batch.saturating_add(1);
    reopened.attempt_generation = reopened.attempt_generation.saturating_add(1);
    reopened.result = None;
    reopened.wait_reason = None;

    let current = reopened
        .slot_name
        .as_deref()
        .and_then(|name| {
            control
                .current_for_name(&route.project_scope_id, &route.app_scope_id, name)
                .ok()
                .flatten()
        });
    if let Some(slot) = current.filter(|slot| slot.active_binding().is_some()) {
        reopened.slot_name = Some(slot.identity_key.name.clone());
        reopened.binding_generation = slot.generation;
        reopened.status = TicketPhaseStatus::Claimed;
    } else {
        reopened.slot_name = None;
        reopened.binding_generation = 0;
        reopened.status = TicketPhaseStatus::WaitingAssignment;
    }

    Some(Event::TicketPhaseRecorded { phase: reopened })
}

/// Same continuation planner for callers that hold an uncommitted publish
/// batch. The new task and its board details exist only as pending events, so
/// the planner must see them explicitly or the first enqueue can never claim.
pub(crate) fn ticket_continuation_events_with_pending(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    worker_id: &str,
    pending: &[crate::server::ticket_state::TicketPhaseRecord],
    pending_task: Option<&TaskRec>,
    pending_details: Option<&crate::board::BoardTaskDetails>,
    trusted_idle: bool,
) -> Vec<Event> {
    use crate::server::ticket_state::{
        next_queued_for_slot, TicketPhaseKind, TicketPhaseRecord, TicketPhaseStatus,
    };

    if current_master_worker_id(state, Some(route)).as_deref() == Some(worker_id) {
        return Vec::new();
    }
    let slot = match control.current_for_worker(&route.project_scope_id, &route.app_scope_id, worker_id) {
        Ok(Some(slot)) => slot,
        _ => return Vec::new(),
    };
    let Some(current) = slot.active_binding() else {
        return Vec::new();
    };
    let Some(session_id) = current.binding.session_id.as_ref() else {
        return Vec::new();
    };
    let slot_name = slot.identity_key.name.clone();
    let binding_generation = slot.generation;

    let owned;
    let phases: &[TicketPhaseRecord] = if pending.is_empty() {
        &state.ticket_phases
    } else {
        let mut overlay = state.ticket_phases.clone();
        for record in pending {
            match overlay
                .iter_mut()
                .find(|existing| {
                    existing.task_id == record.task_id
                        && existing.phase == record.phase
                        && existing.batch == record.batch
                }) {
                Some(existing) => *existing = record.clone(),
                None => overlay.push(record.clone()),
            }
        }
        owned = overlay;
        &owned
    };

    // One active claim per slot. A claimed or running phase already occupies
    // the slot's execution capacity.
    if phases.iter().any(|phase| {
        phase.slot_name.as_deref() == Some(slot_name.as_str())
            && matches!(
                phase.status,
                TicketPhaseStatus::Claimed | TicketPhaseStatus::Running
            )
    }) {
        return Vec::new();
    }

    let idle = trusted_idle
        || state
            .agent_activity
            .get(route.project_scope_id.as_str())
            .and_then(|workers| workers.get(worker_id))
            .is_some_and(|activity| {
                activity.activity == name_state::AgentActivity::Idle
                    && activity.idle_since_ms.is_some()
            });
    if !idle {
        return Vec::new();
    }

    let dependency_satisfied = |dep: &str| {
        state.tasks.get(dep).is_some_and(|task| {
            matches!(
                task.status.as_str(),
                "delivered" | "accepted" | "merged" | "closed"
            )
        }) || phases.iter().any(|phase| {
            phase.task_id == dep
                && phase.phase == TicketPhaseKind::Execute
                && phase.status == TicketPhaseStatus::Completed
        })
    };
    let Some(next) = next_queued_for_slot(phases, &slot_name, dependency_satisfied).cloned() else {
        return Vec::new();
    };
    let pending_matches = pending_task.is_some_and(|task| task.id == next.task_id);
    let Some(mut task) = pending_task
        .filter(|_| pending_matches)
        .cloned()
        .or_else(|| state.tasks.get(&next.task_id).cloned())
    else {
        return Vec::new();
    };
    let policy = pending_details
        .filter(|_| pending_matches)
        .map(|details| details.work_policy.clone())
        .unwrap_or_else(|| state.effective_work_policy(&next.task_id));
    if next.phase == TicketPhaseKind::Execute
        && execution_candidate_eligibility(&policy, &slot_name).is_err()
    {
        return Vec::new();
    }
    let recovered_phase = ticket_recovery_claim_phase(
        state,
        &slot_name,
        session_id.as_str(),
        &next.task_id,
        worker_id,
        binding_generation,
    )
    .filter(|phase| {
        phase.batch == next.batch
            && phase.phase == TicketPhaseKind::Execute
            && phase.status == TicketPhaseStatus::Queued
    });
    let recovered = recovered_phase.is_some();
    if !recovered && (task.status != "pending" || task.owner != task.created_by) {
        return Vec::new();
    }
    if recovered {
        // Reuse the same executor and resource admission as the manual claim
        // path. The planner has no error channel, so an unsafe claim simply
        // emits no events and leaves the queued responsibility intact.
        if board_resume_gate(state, worker_id, &next.task_id).is_err() {
            return Vec::new();
        }
    }
    let Some(mut details) = pending_details
        .filter(|_| pending_matches)
        .cloned()
        .or_else(|| state.board_details.get(&next.task_id).cloned())
    else {
        return Vec::new();
    };

    let mut claimed = recovered_phase.unwrap_or_else(|| next.clone());
    claimed.status = TicketPhaseStatus::Claimed;
    claimed.slot_name = Some(slot_name.clone());
    claimed.binding_generation = binding_generation;
    claimed.attempt_generation = match claimed.attempt_generation.checked_add(1) {
        Some(attempt) => attempt,
        None => return Vec::new(),
    };
    claimed.wait_reason = None;

    if !recovered {
        task.owner = worker_id.to_owned();
        task.status = "working".into();
        task.updated_ms = now_ms();
    }
    details.revision += 1;

    let assignment = name_state::TaskAssignment {
        task_id: next.task_id.clone(),
        name: slot_name.clone(),
        session_id: session_id.as_str().into(),
        assignment_revision: state
            .task_assignment(&next.task_id)
            .map_or(1, |assignment| assignment.assignment_revision + 1),
        reflow: None,
        updated_ms: now_ms(),
    };
    let mut events = vec![Event::TicketPhaseRecorded {
        phase: claimed.clone(),
    }];
    if !recovered {
        events.push(Event::TaskUpdated { task });
    }
    events.extend([
        Event::BoardDetailsChanged {
            task_id: next.task_id.clone(),
            details,
        },
        Event::TaskAssignmentUpdated { assignment },
    ]);
    // The execution contract message is saved in the same transaction as the
    // claim, so a delivered wake always has a committed claim behind it.
    let message_id = format!("ticket-claim-{}", rand::random::<u128>());
    events.push(Event::Sent {
        msg: Message {
            id: message_id.clone(),
            from: "daemon".into(),
            to: worker_id.to_owned(),
            mtype: "notification".into(),
            subject: Some(format!("ticket-claimed:{}", next.task_id)),
            body: format!(
                "Ticket {} is queued on your slot and claimed for you at attempt {}. Run collab board start --attempt {} before working, then submit the result with the same attempt.",
                next.task_id, claimed.attempt_generation, claimed.attempt_generation
            ),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    });
    events.push(Event::DeliveryMode {
        msg_id: message_id.clone(),
        mode: "explicit-notification".into(),
        source_thread_id: None,
    });
    events.push(Event::MailboxNameAddressed {
        message_id: message_id.clone(),
        address: name_mailbox::MailboxNameAddress {
            project_scope: route.project_scope_id.as_str().into(),
            name: slot_name.clone(),
        },
    });
    if let Some(subscription) = state.matching_subscription(worker_id, "direct-message", None, now_ms())
    {
        events.push(Event::WakeBound {
            message_id,
            subscription_id: subscription.id.clone(),
        });
    }
    events
}

/// Re-run the single continuation entry for every idle slot whose queued
/// phases were waiting on this completed execute phase. The delivering actor's
/// own slot is handled by its caller to avoid a duplicate claim in one batch.
pub(crate) fn ticket_dependency_release_events(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    completed: &crate::server::ticket_state::TicketPhaseRecord,
    exclude_worker: &str,
) -> Vec<Event> {
    use crate::server::ticket_state::TicketPhaseStatus;

    let mut slots: Vec<String> = state
        .ticket_phases
        .iter()
        .filter(|phase| {
            phase.status == TicketPhaseStatus::Queued
                && phase.depends_on.iter().any(|dep| dep == &completed.task_id)
        })
        .filter_map(|phase| phase.slot_name.clone())
        .collect();
    slots.sort();
    slots.dedup();

    let mut events = Vec::new();
    for slot_name in slots {
        let worker_id = control
            .current_for_name(&route.project_scope_id, &route.app_scope_id, &slot_name)
            .ok()
            .flatten()
            .and_then(|slot| {
                slot.active_binding()
                    .map(|current| current.binding.agent_id.as_str().to_owned())
            });
        let Some(worker_id) = worker_id else { continue };
        if worker_id == exclude_worker {
            continue;
        }
        events.extend(ticket_continuation_events(
            state,
            control,
            route,
            &worker_id,
            std::slice::from_ref(completed),
            false,
        ));
    }
    events
}

#[cfg(test)]
mod work_policy_state_tests {
    use super::*;

    #[test]
    fn category_defaults_overrides_and_explicit_clear_survive_current_state() {
        let mut state = State::default();
        state
            .apply_checked(&Event::ProjectWorkPolicyUpdated {
                work_kind: "frontend".into(),
                policy: parse_work_policy(
                    Some("frontend".into()),
                    Some("designer".into()),
                    Some("Alice".into()),
                    Some("Bob".into()),
                    false,
                    false,
                    false,
                )
                .unwrap(),
            })
            .unwrap();
        let goal = crate::board::BoardTaskDetails {
            work_policy: crate::board::WorkPolicy {
                work_kind: Some("frontend".into()),
                ..Default::default()
            },
            child_work_policies: std::collections::BTreeMap::from([
                (
                    "frontend".into(),
                    parse_work_policy(None, None, Some("Bob".into()), None, false, false, false)
                        .unwrap(),
                ),
                (
                    "backend".into(),
                    parse_work_policy(None, None, Some("Carol".into()), None, false, false, false)
                        .unwrap(),
                ),
            ]),
            ..Default::default()
        };
        state
            .apply_checked(&Event::BoardDetailsChanged {
                task_id: "goal".into(),
                details: goal,
            })
            .unwrap();
        let override_policy =
            parse_work_policy(None, None, Some("Alice".into()), None, true, false, false).unwrap();
        let resolved = state.materialize_work_policy(None, Some("goal"), &override_policy);
        assert_eq!(resolved.executor_name.as_deref(), Some("Alice"));
        assert_eq!(resolved.reviewer_name.as_deref(), Some("Bob"));
        assert!(resolved.preferred_roles.is_none());
        assert_eq!(
            state
                .materialize_work_policy(Some("backend"), Some("goal"), &Default::default())
                .executor_name
                .as_deref(),
            Some("Carol")
        );
        state
            .apply_checked(&Event::BoardDetailsChanged {
                task_id: "child".into(),
                details: crate::board::BoardTaskDetails {
                    parent_task_id: Some("goal".into()),
                    work_policy: resolved.clone(),
                    ..Default::default()
                },
            })
            .unwrap();
        let mut replay: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(replay.effective_work_policy("child"), resolved);
        assert_eq!(
            replay
                .materialize_work_policy(Some("frontend"), None, &Default::default())
                .reviewer_name
                .as_deref(),
            Some("Bob")
        );
        replay.project_work_policy.clear();
        assert_eq!(
            replay.effective_work_policy("child"),
            resolved,
            "saved tasks never re-resolve changed defaults"
        );
    }
}
