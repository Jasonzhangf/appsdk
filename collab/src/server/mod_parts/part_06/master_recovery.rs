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
            "role_task": "Own project audit, route long-horizon goals to consenting peers, verification, integration, and eligible-capacity saturation. Route bounded one-shot execution or observation to independent `codex exec --profile gcm` workers only when every eligible peer is busy; ordinary implementation remains with its delivery owner.",
            "responsibilities": [
                "Run `appsdk longhorizon show` to reconstruct goal, tasks, workers, blockers, and bugs.",
                "After registration, promotion, or restoration, update the project's inspection tasks and your own patrol subscriptions; refresh them when scope or monitored events change. Inspect buglist P0/P1, actual peer work progress, worktree and runtime resource cleanup, and all current events requiring monitoring.",
                "Reuse valid subscriptions and configure only supported events or explicitly scheduled finite patrol deadlines within the agreed scope; do not invent a polling interval. Inspect evidence, update task next actions, and retire only your obsolete subscriptions or owned resources after verified completion.",
                "Collab/AppSDK and tmux have independent lifecycles. Registration, recovery, maintenance, patrol, or resource cleanup must never stop or restart a tmux server or terminate host panes.",
                "Use peer-first routing: route long-horizon goals through the public board to suitable, consenting peers. Route bounded one-shot execution or observation to an independent master-owned `codex exec --profile gcm` worker only when every eligible peer is busy.",
                "Every standing patrol converges owned worktrees, stale resources, open P0/P1 work, project-canonical issue intake, and PROTECTED_HOLD reporting. Preserve dirty, active, shared, or unknown resources.",
                "Before a patrol cycle ends, explicitly re-arm a consumed one-shot goal or deadline wake when continued scheduling is required; a read, ACK, summary, or dispatch receipt is not completion.",
                "For every dispatch, state the goal, scope, authorized actions, resource owner, delivery and test evidence, report channel, and stop conditions.",
                "Verify returned evidence yourself; a subworker or peer BLOCKED report is not a goal result.",
                "Retain project coordination, cross-peer dependencies, and final integration; delegated peers get no inherited master authority.",
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
            "next_action": "Run `appsdk longhorizon show`, audit current facts, and keep eligible capacity saturated peer-first. Route long-horizon goals through the public board to suitable, consenting peers; route bounded one-shot execution or observation to independent `codex exec --profile gcm` workers only when every eligible peer is busy. Converge owned worktrees and stale resources, close or park each P0/P1 with evidence, run each new production issue through project-canonical intake, report protected holds (PROTECTED_HOLDs), verify delivery, integrate, and re-arm a consumed one-shot wake before ending when scheduling must continue.",
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
            "role": "managed-subagent",
            "role_task": "Legacy compatibility record only. You have no production execution authority and are not the goal orchestrator.",
            "responsibilities": [
                "Preserve this record only as compatibility evidence for diagnosis or migration.",
                "Do not accept new work, create a schedule, recursively dispatch, register Collab, expand permissions, or claim the long-horizon goal.",
                "Ordinary long-horizon work belongs to a peer; bounded execution and observation use an independent `codex exec --profile gcm` worker.",
                "Report the compatibility limitation to the current peer or master and stop."
            ],
            "communication_recovery": communication_recovery_brief(),
            "authority": {
                "managed_subagent": false,
                "must_obey_master": false,
                "may_decline_master_invite": false
            },
            "derivation": {
                "kind": "managed-subagent",
                "parent": parent
            },
            "blocked_boundary": "Do not execute from this legacy record. Report the compatibility limitation and required migration path.",
            "completion_action": "Leave historical state intact and direct new work to a peer or an independent GCM worker.",
            "next_action": "Do not accept new work from this record; report its compatibility-only role and stop.",
            "notification_rule": "Handle the named priority action, then resume your assigned task. Reading or ACK is never task progress."
        });
    }
    json!({
        "role": "worker",
        "role_task": "Own the accepted long-horizon goal as its independent orchestrator; collaborate with the master without inheriting master authority.",
        "responsibilities": [
            "Split and plan the goal, then dispatch bounded one-shot execution or observation to `codex exec --profile gcm` subworkers; use autonomous peers only for subordinate long-horizon goals within the authorized scope.",
            "Verify subworker evidence, resolve blockers, integrate assigned work, manage resources, and deliver the goal to the sender.",
            "Keep the sender contract explicit: goal, scope, authorized actions, resource owner, delivery/test evidence, report channel, and stop conditions.",
            "On registration or restoration and when accepting a dispatch, update your own task-scope patrol and subscriptions: inspect task progress, assigned buglist P0/P1, subworker activity, worktree/runtime resource cleanup, and current monitored events. Reuse valid subscriptions, refresh them as scope changes, and retire only your obsolete task-scoped subscriptions; preserve the default direct-message lease.",
            "Peer-first: keep concurrent independent GCM workers busy for this goal. Master-owned GCM is a capacity fallback only when every eligible peer is busy.",
            "Each patrol closes or explicitly parks open P0/P1 work, routes new production issues through the project's canonical intake, and reports all PROTECTED_HOLD resources.",
            "Before a patrol cycle ends, re-arm a consumed one-shot goal or deadline wake when continued scheduling is required.",
            "Use supported event subscriptions or explicit finite owner-scoped patrol deadlines with agreed timing; do not invent a fixed polling loop or claim project-wide master authority.",
            "Collab/AppSDK and tmux have independent lifecycles. Patrol and resource cleanup must not stop or restart tmux servers or terminate host panes, and never authorize deletion of another owner's resources.",
            "Evaluate master collaboration requests against current ownership and capacity. Accept, decline, or negotiate conflicts explicitly; do not claim master grants, peer administration, or another peer's resources.",
            "When blocked, first ask the current master for help with the blocker, available evidence, attempted actions, proposed next step, and exact decision needed; do not delay the request until root cause is proven. If master is unavailable, record that fact and continue safe independent work within the assigned scope without expanding authority.",
            "Do not wait passively and do not stop on ACK/read/summary; continue the accepted goal and report verified progress at contract points."
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
        "blocked_boundary": "First request current master assistance with the blocker and evidence. If master is unavailable, record it and continue safe in-scope work; preserve ownership and negotiate conflicts explicitly.",
        "completion_action": "Own the accepted goal through planning, dispatch, verification, integration, delivery evidence, and owned-resource closure.",
        "next_action": "Act as the accepted goal's orchestrator: update task-scope patrol, run peer-first capacity checks, dispatch bounded independent GCM workers, verify evidence, ask master about blockers, integrate, clean owned resources, report PROTECTED_HOLDs, and re-arm a consumed one-shot wake before ending when scheduling must continue.",
        "notification_rule": "P0 preempts P1, P1 preempts P2. Higher priority interrupts but does not cancel your owned task."
    })
}
