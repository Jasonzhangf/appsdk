use super::name_state::{AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord};
use super::observation_epoch::first_observation_since_start;
use super::state::Event;
use super::{GlobalState, Server};
use crate::client::adapters::tmux::{self, AgentObservation};
use crate::proto::{SelectedTransport, TmuxEndpoint, TransportKind, WorkerStatusField};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TmuxExecutionTarget {
    key: super::binding_slot::IdentityBindingKey,
    generation: u64,
    binding: super::RuntimeBinding,
    endpoint: TmuxEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ObservedTmuxExecutionRecord {
    target: TmuxExecutionTarget,
    execution: tmux::ObservedTmuxExecution,
}

impl ObservedTmuxExecutionRecord {
    pub(super) fn matches_state(&self, state: &super::State, worker_id: &str) -> bool {
        self.execution.pid != 0
            && !self.execution.start_id.is_empty()
            && self.target.generation != 0
            && self.target.key.project_scope == self.target.binding.project_scope
            && self.target.key.app_scope_id == self.target.binding.app_scope_id
            && self.target.binding.agent_id.as_str() == worker_id
            && state
                .global
                .lookup_project(&self.target.binding.project_scope)
                .and_then(|project| {
                    project
                        .runtime_bindings
                        .get(self.target.binding.binding_id.as_str())
                })
                == Some(&self.target.binding)
            && state
                .workers
                .get(worker_id)
                .and_then(|worker| worker.transport.as_ref())
                .and_then(observation_target)
                .is_some_and(|(session, endpoint)| {
                    self.target
                        .binding
                        .session_id
                        .as_ref()
                        .map(|id| id.as_str())
                        == Some(session)
                        && endpoint == &self.target.endpoint
                })
    }
}

fn current_target(
    control: &super::binding_slot::BindingControlSet,
    state: &super::State,
    scope: &str,
    worker_id: &str,
) -> Option<TmuxExecutionTarget> {
    let transport = state.workers.get(worker_id)?.transport.as_ref()?;
    let (session, endpoint) = observation_target(transport)?;
    let mut slots = control.slots().filter(|slot| {
        slot.identity_key.project_scope.as_str() == scope
            && slot.active_binding().is_some_and(|value| {
                value.binding.agent_id.as_str() == worker_id
                    && &value.transport == transport
                    && state
                        .global
                        .lookup_project(&value.binding.project_scope)
                        .and_then(|project| {
                            project
                                .runtime_bindings
                                .get(value.binding.binding_id.as_str())
                        })
                        == Some(&value.binding)
            })
    });
    let slot = slots.next()?;
    if slots.next().is_some() {
        return None;
    }
    let value = slot.active_binding()?;
    if value.binding.session_id.as_ref().map(|id| id.as_str()) != Some(session) {
        return None;
    }
    Some(TmuxExecutionTarget {
        key: slot.identity_key.clone(),
        generation: slot.generation(),
        binding: value.binding.clone(),
        endpoint: endpoint.clone(),
    })
}

fn with_state<R>(
    manager: Option<&super::ProjectRuntimeManager>,
    server: &Server,
    operation: impl FnOnce(
        &super::binding_slot::BindingControlSet,
        &mut super::State,
    ) -> Result<R, String>,
) -> Result<R, String> {
    if let Some(manager) = manager {
        manager.with_tmux_monitor_state(server, operation)
    } else {
        let control =
            super::binding_store::BindingControlStore::new(server.host_paths.state_root())
                .load()?;
        let mut state = server.state.lock().unwrap();
        operation(&control, &mut state)
    }
}

pub(super) fn missing_status_fields(
    presence: Option<&AgentPresenceRecord>,
    activity: Option<&AgentActivityRecord>,
) -> Vec<WorkerStatusField> {
    let mut missing = Vec::new();
    if presence.is_none_or(|record| record.presence == AgentPresence::Unknown) {
        missing.push(WorkerStatusField::Presence);
    }
    let offline = presence.is_some_and(|record| record.presence == AgentPresence::Offline);
    if !offline && activity.is_none_or(|record| record.activity == AgentActivity::Unknown) {
        missing.push(WorkerStatusField::Activity);
    }
    missing
}

pub(super) fn tick(server: &Server, now: i64) {
    tick_internal(None, server, now, &tmux::observe_agent_with_execution);
}

pub(super) fn tick_managed(manager: &super::ProjectRuntimeManager, server: &Server, now: i64) {
    tick_internal(
        Some(manager),
        server,
        now,
        &tmux::observe_agent_with_execution,
    );
}

pub(super) fn tick_with_observer(
    server: &Server,
    now: i64,
    observe: &dyn Fn(&TmuxEndpoint) -> Result<AgentObservation, String>,
) {
    tick_internal(None, server, now, &|endpoint, _| {
        observe(endpoint).map(|observation| tmux::ExecutionObservation {
            observation,
            execution: None,
        })
    });
}

fn tick_internal(
    manager: Option<&super::ProjectRuntimeManager>,
    server: &Server,
    now: i64,
    observe: &dyn Fn(
        &TmuxEndpoint,
        Option<&tmux::ObservedTmuxExecution>,
    ) -> Result<tmux::ExecutionObservation, String>,
) {
    let scope = match GlobalState::canonical_project_scope(&server.root) {
        Ok(scope) => scope,
        Err(error) => {
            super::presence::append_log(
                &server.log_path(),
                &format!("NAME_MONITOR_SCOPE_UNKNOWN: {error}"),
            );
            return;
        }
    };
    let workers: Vec<_> = server
        .state
        .lock()
        .unwrap()
        .workers
        .values()
        .cloned()
        .collect();
    for worker in workers {
        let Some(transport) = worker.transport.as_ref() else {
            continue;
        };
        let Some((session, endpoint)) = observation_target(transport) else {
            continue;
        };
        let prepared = with_state(manager, server, |control, state| {
            let target = current_target(control, state, scope.as_str(), &worker.id);
            let baseline = state.tmux_executions.get(&worker.id).cloned();
            Ok((target, baseline))
        });
        let (target, baseline) = match prepared {
            Ok(value) => value,
            Err(error) => {
                super::presence::append_log(
                    &server.log_path(),
                    &format!("TMUX_MONITOR_PREPARE_FAILED: {error}"),
                );
                continue;
            }
        };
        if manager.is_some() && target.is_none() {
            continue;
        }
        let previous = baseline
            .as_ref()
            .filter(|record| Some(&record.target) == target.as_ref())
            .map(|record| &record.execution);
        // Both State and register_gate are released before any host probe.
        let result = observe(endpoint, previous);
        let (observed, execution) = match result {
            Ok(result) => (result.observation, result.execution),
            Err(reason) => (
                AgentObservation {
                    agent_presence: tmux::AgentPresence::Unknown,
                    activity: tmux::AgentActivity::Unknown,
                    background: false,
                    foreground_command: None,
                    reason,
                },
                None,
            ),
        };
        let mut presence = AgentPresenceRecord {
            project_scope: scope.as_str().into(),
            worker_id: worker.id.clone(),
            session_id: session.into(),
            presence: match observed.agent_presence {
                tmux::AgentPresence::Online => AgentPresence::Online,
                tmux::AgentPresence::Offline => AgentPresence::Offline,
                tmux::AgentPresence::Unknown => AgentPresence::Unknown,
            },
            reason: observed.reason.clone(),
            observed_ms: now,
        };
        let old_activity = {
            let state = server.state.lock().unwrap();
            state
                .agent_activity
                .get(scope.as_str())
                .and_then(|workers| workers.get(&worker.id))
                .cloned()
        };
        let activity_kind = match observed.activity {
            tmux::AgentActivity::Busy => AgentActivity::Busy,
            tmux::AgentActivity::Idle => AgentActivity::Idle,
            tmux::AgentActivity::WaitingUser => AgentActivity::WaitingUser,
            tmux::AgentActivity::Error => AgentActivity::Error,
            tmux::AgentActivity::Unknown => AgentActivity::Unknown,
        };
        let positive_online_idle =
            presence.presence == AgentPresence::Online && activity_kind == AgentActivity::Idle;
        let idle_since_ms = if positive_online_idle {
            // A trusted interval must not survive the observation boundary
            // after a daemon restart: the first live sample starts a fresh one.
            let is_first =
                first_observation_since_start(&server.root, &format!("activity:{}", worker.id));
            match old_activity.as_ref() {
                Some(old)
                    if !is_first
                        && old.session_id == presence.session_id
                        && old.activity == AgentActivity::Idle
                        && old.observed_ms <= now
                        && old.idle_since_ms.is_some_and(|start| now >= start) =>
                {
                    old.idle_since_ms
                }
                _ => Some(now),
            }
        } else {
            None
        };
        let activity = AgentActivityRecord {
            project_scope: scope.as_str().into(),
            worker_id: worker.id.clone(),
            session_id: session.into(),
            activity: activity_kind,
            background: observed.background,
            reason: observed.reason,
            observed_ms: now,
            idle_since_ms,
        };
        let committed = with_state(manager, server, |control, state| {
            if state.tmux_executions.get(&worker.id) != baseline.as_ref()
                || current_target(control, state, scope.as_str(), &worker.id) != target
            {
                return Ok(());
            }
            let current = state
                .workers
                .get(&worker.id)
                .and_then(|worker| worker.transport.as_ref())
                .and_then(observation_target);
            if current != Some((session, endpoint)) {
                return Ok(());
            }
            let mut events = Vec::new();
            if let (Some(target), Some(execution)) = (target.clone(), execution) {
                let record = ObservedTmuxExecutionRecord { target, execution };
                if baseline.as_ref() != Some(&record) {
                    events.push(Event::TmuxExecutionObserved {
                        worker_id: worker.id.clone(),
                        record,
                    });
                }
            }
            let old_presence = state
                .agent_presence
                .get(scope.as_str())
                .and_then(|workers| workers.get(&worker.id));
            if old_presence.is_none_or(|old| {
                old.session_id != presence.session_id
                    || old.presence != presence.presence
                    || old.reason != presence.reason
            }) {
                if presence.presence != AgentPresence::Unknown {
                    if let Some(old) = old_presence {
                        // Distinguish a deterministic sample from a verdict
                        // even when both arrive in the same millisecond.
                        presence.observed_ms = presence.observed_ms.max(old.observed_ms + 1);
                    }
                }
                events.push(Event::AgentPresenceObserved { presence });
            }
            if old_activity.as_ref().is_none_or(|old| {
                old.session_id != activity.session_id
                    || old.activity != activity.activity
                    || old.background != activity.background
                    || old.reason != activity.reason
                    || old.idle_since_ms != activity.idle_since_ms
            }) {
                events.push(Event::AgentActivityObserved { activity });
            }
            // Keep the existing online-idle ticket continuation owner.
            if positive_online_idle {
                if let Ok(Some(route)) = super::server_route_scope(server, state) {
                    events.extend(super::ticket_continuation_events(
                        state,
                        control,
                        &route,
                        &worker.id,
                        &[],
                        true,
                    ));
                }
            }
            if !events.is_empty() {
                server
                    .commit_locked_checked_with_control(state, &events, control)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        });
        if let Err(error) = committed {
            super::presence::append_log(
                &server.log_path(),
                &format!("TMUX_MONITOR_COMMIT_FAILED: {error}"),
            );
        }
    }
}

fn observation_target(transport: &SelectedTransport) -> Option<(&str, &TmuxEndpoint)> {
    if transport.kind != TransportKind::Tmux {
        return None;
    }
    let endpoint = transport.tmux_endpoint.as_ref()?;
    let session = transport
        .session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            endpoint
                .codex_session_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })?;
    Some((session, endpoint))
}

#[cfg(test)]
mod tests;
