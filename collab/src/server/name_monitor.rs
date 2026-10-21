use super::name_state::{AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord};
use super::state::Event;
use super::{GlobalState, Server};
use crate::client::adapters::tmux::{self, AgentObservation};
use crate::proto::{SelectedTransport, TmuxEndpoint, TransportKind};

pub(super) fn tick(server: &Server, now: i64) {
    tick_with_observer(server, now, &tmux::observe_agent);
}

pub(super) fn tick_with_observer(
    server: &Server,
    now: i64,
    observe: &dyn Fn(&TmuxEndpoint) -> Result<AgentObservation, String>,
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
        // The external probe must never hold the reducer lock.
        let observed = match observe(endpoint) {
            Ok(observed) => observed,
            Err(reason) => AgentObservation {
                agent_presence: tmux::AgentPresence::Unknown,
                activity: tmux::AgentActivity::Unknown,
                background: false,
                foreground_command: None,
                reason,
            },
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
        let activity = AgentActivityRecord {
            project_scope: scope.as_str().into(),
            worker_id: worker.id.clone(),
            session_id: session.into(),
            activity: match observed.activity {
                tmux::AgentActivity::Busy => AgentActivity::Busy,
                tmux::AgentActivity::Idle => AgentActivity::Idle,
                tmux::AgentActivity::WaitingUser => AgentActivity::WaitingUser,
                tmux::AgentActivity::Unknown => AgentActivity::Unknown,
            },
            background: observed.background,
            reason: observed.reason,
            observed_ms: now,
        };
        let mut state = server.state.lock().unwrap();
        let current = state
            .workers
            .get(&worker.id)
            .and_then(|worker| worker.transport.as_ref())
            .and_then(observation_target);
        if current != Some((session, endpoint)) {
            continue;
        }
        let mut events = Vec::new();
        let old_presence = state
            .agent_presence
            .get(scope.as_str())
            .and_then(|workers| workers.get(&worker.id));
        if !super::name_stewardship::retain_adjudicated_presence(&state, &presence)
            && old_presence.is_none_or(|old| {
                old.session_id != presence.session_id
                    || old.presence != presence.presence
                    || old.reason != presence.reason
            })
        {
            if presence.presence != AgentPresence::Unknown {
                if let Some(old) = old_presence {
                    // A deterministic observation after a verdict must be
                    // distinguishable even when both occur in the same ms.
                    presence.observed_ms = presence.observed_ms.max(old.observed_ms + 1);
                }
            }
            events.push(Event::AgentPresenceObserved { presence });
        }
        let old_activity = state
            .agent_activity
            .get(scope.as_str())
            .and_then(|workers| workers.get(&worker.id));
        if old_activity.is_none_or(|old| {
            old.session_id != activity.session_id
                || old.activity != activity.activity
                || old.background != activity.background
                || old.reason != activity.reason
        }) {
            events.push(Event::AgentActivityObserved { activity });
        }
        if !events.is_empty() {
            server.commit_locked_reporting(&mut state, &events);
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
