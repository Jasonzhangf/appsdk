fn server_route_scope(server: &Server, state: &State) -> Result<Option<RouteScope>, &'static str> {
    route_scope_for_root(&server.root, state)
}

fn master_grant_for_worker(
    state: &State,
    route_scope: &RouteScope,
    worker_id: &str,
    granted_by: &str,
    approval: &str,
) -> Result<crate::server::global_state::MasterGrant, String> {
    let project = state
        .global
        .lookup_project_for_route(route_scope)
        .ok_or_else(|| {
            format!(
                "MASTER_AUTHORITY_REQUIRES_REGISTERED_ROUTE: {} / {}",
                route_scope.project_scope_id.as_str(),
                route_scope.app_scope_id
            )
        })?;
    let mut bindings = project.runtime_bindings.values().filter(|binding| {
        binding.project_scope == route_scope.project_scope_id
            && binding.app_scope_id == route_scope.app_scope_id
            && binding.agent_id.as_str() == worker_id
    });
    let Some(binding) = bindings.next() else {
        return Err(format!(
            "MASTER_AUTHORITY_REQUIRES_RUNTIME_BINDING: worker {worker_id} has no runtime binding"
        ));
    };
    if bindings.next().is_some() {
        return Err(format!(
            "MASTER_AUTHORITY_AMBIGUOUS_BINDING: worker {worker_id} has multiple runtime bindings"
        ));
    }
    crate::server::global_state::MasterGrant::new(
        binding.project_scope.clone(),
        binding.app_scope_id.clone(),
        binding.agent_id.clone(),
        "project",
        granted_by,
        approval,
        binding.binding_id.clone(),
        binding.endpoint_generation,
        now_ms(),
    )
    .map_err(|error| error.to_string())
}

fn master_authority_transfer_events(
    state: &State,
    route_scope: &RouteScope,
    grant: crate::server::global_state::MasterGrant,
) -> Vec<Event> {
    let mut events = master_authority_revoke_events(state, route_scope);
    events.push(Event::GlobalMasterGranted { grant });
    events
}

/// Revoke every current typed grant in the exact route scope without issuing a
/// replacement. `promote` reuses the same scoped revoke set and then grants, so
/// promote, delegate and clear share one authority-change owner.
fn master_authority_revoke_events(state: &State, route_scope: &RouteScope) -> Vec<Event> {
    state
        .global
        .lookup_project_for_route(route_scope)
        .into_iter()
        .flat_map(|project| project.master_grants.values())
        .filter(|current| {
            current.project_scope == route_scope.project_scope_id
                && current.app_scope_id == route_scope.app_scope_id
        })
        .map(|current| Event::GlobalMasterRevoked {
            project_scope: current.project_scope.clone(),
            binding_id: current.binding_id.clone(),
        })
        .collect()
}

/// Finds the bindings that an incoming registration reclaims.
///
/// A tmux pane has one owner, and the pane id is the whole anchor a tmux
/// registration fixes at registration time. The later registrant therefore
/// replaces every other binding that holds the same pane on the same tmux
/// server, in any project or app scope. Nothing is probed: a pane can never
/// answer a liveness question.
fn pane_claimants(
    state: &State,
    taker: &str,
    route_scope: &RouteScope,
    endpoint: &crate::proto::TmuxEndpoint,
) -> Vec<RuntimeBinding> {
    state
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .filter(|binding| {
            binding.tmux_endpoint.as_ref().is_some_and(|previous| {
                crate::client::adapters::tmux::same_owned_pane(previous, endpoint)
            })
        })
        // The binding this registration is about to own is not a takeover:
        // a same-scope re-registration stays idempotent.
        .filter(|binding| {
            !(binding.agent_id.as_str() == taker && binding.route_scope() == *route_scope)
        })
        .cloned()
        .collect()
}

/// Events that hand one pane to a later registrant.
///
/// The previous claimant loses the pane, its current-thread route and its worker
/// record in the same commit as the new binding, so the ledger cannot fence the
/// new owner on the next attempt.
fn pane_reclaim_events(taker: &str, previous: &RuntimeBinding) -> Result<Vec<Event>, String> {
    let next_generation = previous.endpoint_generation.checked_add(1).ok_or_else(|| {
        "RUNTIME_BINDING_REJECTED: reclaimed anchor generation overflow".to_owned()
    })?;
    let mut retired = previous.clone();
    retired.endpoint_generation = next_generation;
    retired.native_thread_id = None;
    retired.tmux_endpoint = None;
    let mut events = vec![
        Event::GlobalCurrentThreadRouteRetired {
            binding: previous.clone(),
        },
        Event::GlobalRuntimeBound { binding: retired },
    ];
    if previous.agent_id.as_str() != taker {
        events.push(Event::WorkerClosed {
            worker_id: previous.agent_id.as_str().to_owned(),
            closed_by: taker.to_owned(),
            reason: "tmux pane reclaimed by a later registration".to_owned(),
            snapshot_captured_ms: None,
            at_ms: now_ms(),
        });
    }
    Ok(events)
}

fn current_master_grant(
    state: &State,
    route_scope: Option<&RouteScope>,
) -> Option<crate::server::global_state::MasterGrant> {
    if let Some(route_scope) = route_scope {
        let project = state.global.lookup_project_for_route(route_scope)?;
        let mut grants = project.master_grants.values().filter(|grant| {
            grant.project_scope == route_scope.project_scope_id
                && grant.app_scope_id == route_scope.app_scope_id
                && state
                    .global
                    .lookup_master_grant(&grant.project_scope, &grant.binding_id)
                    .is_some_and(|current| current == *grant)
        });
        let grant = grants.next()?.clone();
        return grants.next().is_none().then_some(grant);
    }
    let mut grants = state
        .global
        .projects
        .values()
        .flat_map(|project| project.master_grants.values())
        .filter(|grant| {
            state
                .global
                .lookup_master_grant(&grant.project_scope, &grant.binding_id)
                .is_some_and(|current| current == *grant)
        });
    let grant = grants.next()?.clone();
    grants.next().is_none().then_some(grant)
}

fn current_master_worker_id(state: &State, route_scope: Option<&RouteScope>) -> Option<String> {
    current_master_grant(state, route_scope).map(|grant| grant.agent_id.as_str().to_owned())
}

/// The current typed grant holder for the server's route, read only from the
/// reducer. Transport liveness is a separate observation and never gates this
/// read, so an unreachable holder still owns the authority it was granted.
pub(crate) fn current_master_holder(
    server: &Server,
    state: &State,
) -> Result<Option<String>, &'static str> {
    let route_scope = server_route_scope(server, state)?;
    Ok(current_master_worker_id(state, route_scope.as_ref()))
}

/// The current grant holder's worker record. This is used where a caller needs
/// the holder's selected transport for scheduling or notifications; it never
/// probes that transport, so a missing probe cannot revoke authority.
fn current_master_worker_record(
    server: &Server,
    state: &State,
) -> Result<Option<WorkerRec>, &'static str> {
    let route_scope = server_route_scope(server, state)?;
    Ok(current_master_worker_id(state, route_scope.as_ref())
        .and_then(|id| state.workers.get(&id).cloned()))
}

fn communication_recovery_brief() -> serde_json::Value {
    json!({
        "on_error": "Preserve the exact communication error and durable IDs; an ACK, notification acceptance, daemon health, or timeout is not delivery.",
        "steps": [
            "Run `collab context` and inspect the named route, identity, daemon, task, and inbox state.",
            "If context returns requires_identity_update, supply only its required_fields once through `collab context --provide '<JSON>'`; the daemon owns identity recovery and binding updates.",
            "If context fails, preserve the exact error and report through a healthy peer or the human. TOKEN_MISMATCH and identity conflicts need owner repair; do not choose another worker, copy credentials, edit routes, or start a second daemon."
        ],
        "close_only_when": [
            "the same native target produces a result item",
            "the durable receipt for that result is consumed through the canonical receive/consume operation; read-only inspection alone does not close",
            "the bug or feature record is updated with the full evidence"
        ]
    })
}

fn role_brief(server: &Server, state: &State, worker_id: &str) -> serde_json::Value {
    let route_scope = server_route_scope(server, state).ok().flatten();
    if current_master_worker_id(state, route_scope.as_ref()).as_deref() == Some(worker_id) {
        return json!({
            "role": "master",
            "role_task": "Orchestrate the project; implementation is not your primary job.",
            "responsibilities": [
                "Run `appsdk longhorizon show` to reconstruct goal, tasks, workers, blockers, and bugs.",
                "Split work into independent scopes; assign tasks and resources; keep useful worker capacity loaded.",
                "Assign long-horizon work to an eligible peer first; that peer orchestrates concurrent independent `codex exec --profile gcm` workers. Start a master-owned GCM worker only when every eligible peer is busy.",
                "On every patrol, remove only unneeded owned worktrees after recoverable extraction; reclaim stale owned resources; close each open P0/P1 with applicable evidence or explicitly park it with owner, cause, and unblock condition; route each new production issue through the project's canonical intake before repair; preserve protected dirty/active/shared/unknown resources. Rearm a consumed one-shot goal/deadline wake before the patrol ends when scheduling continues. Read, ACK, status, or dispatch receipt is never completion.",
                "Delivery, merge, or a review verdict is not a lifecycle endpoint; drive review/integration/cleanup/close and assign the next ready P0/P1 task.",
                "Own worker blockers: investigate, unblock, reassign, or close. Do not wait for someone else.",
                "Drive test, verification, commit, merge, worktree cleanup, and task closure.",
                "Continue under the standing goal without waiting for user input; hold wakes only for a true external approval or dependency gate."
            ],
            "communication_recovery": communication_recovery_brief(),
            "authority": {
                "managed_subagent": false,
                "must_obey_master": false,
                "may_decline_master_invite": true
            },
            "derivation": {
                "kind": "project-master",
                "parent": null
            },
            "blocked_boundary": "Investigate and unblock first; only pause for a true external approval or dependency gate.",
            "completion_action": "Drive the project to verified merge, cleanup, task closure, and final acceptance.",
            "next_action": "Run `appsdk longhorizon show`; assign long-horizon work to eligible peers first, keep eligible capacity saturated, verify and integrate delivered work, execute every patrol obligation, and rearm consumed one-shot wakes before ending the patrol. Delivery or review is not an endpoint.",
            "notification_rule": "A notification is an interrupt, not completion. Do its P0/P1/P2 action, then resume scheduling; never stop on ACK/read/summary."
        });
    }
    if is_managed_subagent(state, worker_id) {
        let parent = state
            .subagents
            .values()
            .find(|record| record.peer == worker_id)
            .map(|record| record.parent.clone());
        return json!({
            "role": "legacy-managed-compatibility",
            "role_task": "This is a retained legacy managed record, not an active execution or dispatch surface.",
            "responsibilities": [
                "Preserve the record, its journal, mailbox, worktree, and evidence ownership until an explicit owner-scoped migration or retirement.",
                "Do not start new production work, claim public board tasks, invite peers, or create managed children.",
                "Inspect the record through the owner's compatibility commands and report migration or retirement needs to the recorded parent or current master."
            ],
            "communication_recovery": communication_recovery_brief(),
            "authority": {
                "managed_subagent": true,
                "must_obey_master": false,
                "may_decline_master_invite": true,
                "may_start_new_work": false,
                "may_dispatch": false
            },
            "derivation": {
                "kind": "legacy-managed-compatibility",
                "parent": parent
            },
            "blocked_boundary": "Do not execute or dispatch new work; use an owner-scoped migration or retirement path.",
            "completion_action": "Preserve the record until its owner migrates or retires it.",
            "next_action": "Run `collab context`; do not start new work. Ask the recorded parent or current master for an owner-scoped migration or retirement.",
            "notification_rule": "Handle only the named compatibility or migration action; reading, ACK, or status is not execution progress."
        });
    }
    json!({
        "role": "worker",
        "role_task": "Own and complete your independent task; collaborate with the master without abandoning existing ownership.",
        "responsibilities": [
            "Execute your registered task end to end within its worktree and file scope: implement, test, commit, deliver evidence, and close resources.",
            "When assigned long-horizon work, orchestrate concurrent independent `codex exec --profile gcm` workers inside your task scope; use only the documented fallback when GCM is unavailable.",
            "Evaluate master collaboration requests against current ownership and capacity. Accept ready non-conflicting work; decline or negotiate conflicts explicitly instead of silently ignoring them.",
            "On trouble, investigate first. Report root cause, attempted actions, proposed fix, and the exact decision needed to the live master.",
            "Do not wait passively and do not stop on ACK/read/summary; after handling a notification, resume your current task."
        ],
        "communication_recovery": communication_recovery_brief(),
        "authority": {
            "managed_subagent": false,
            "must_obey_master": false,
            "may_decline_master_invite": true
        },
        "derivation": {
            "kind": "peer",
            "parent": null
        },
        "blocked_boundary": "Protect current ownership and capacity; negotiate conflicts explicitly instead of silently accepting or ignoring them.",
        "completion_action": "Own the task through implementation, verification, delivery evidence, and resource closure.",
        "next_action": "Resume the registered task or remain available for an explicit dispatch.",
        "notification_rule": "P0 preempts P1, P1 preempts P2. Higher priority interrupts but does not cancel your owned task."
    })
}
