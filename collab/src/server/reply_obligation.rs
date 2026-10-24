//! Daemon-owned reply obligations.
//!
//! The obligation record is the only truth. A message body or metadata never
//! creates or clears one; the send owner declares a policy and the linked reply
//! validator clears it. Stable names keep an obligation answerable after the
//! same name moves to a new session.

use super::global_state::GlobalState;
use super::state::{now_ms, ReplyObligation, State};

/// Resolve the project scope that owns a new obligation. A name-addressed send
/// carries its authoritative scope; otherwise the sender's canonical cwd is the
/// owning project.
pub(crate) fn resolve_scope(state: &State, from: &str, name_scope: Option<&str>) -> Option<String> {
    if let Some(scope) = name_scope {
        return Some(scope.to_owned());
    }
    state.workers.get(from).and_then(|worker| {
        GlobalState::canonical_project_scope(std::path::Path::new(&worker.cwd))
            .ok()
            .map(|scope| scope.as_str().to_owned())
    })
}

/// The stable name currently bound to `worker_id`, if any.
pub(crate) fn stable_name(state: &State, worker_id: &str) -> Option<String> {
    state.worker_names.get(worker_id).cloned()
}

fn name_holder<'a>(state: &'a State, project_scope: &str, name: &str) -> Option<&'a str> {
    state
        .name_occupancy(project_scope, name)
        .map(|occupancy| occupancy.worker_id.as_str())
}

/// Verify that `from` is the original recipient (or the current carrier of the
/// recipient name) replying to `to`, the original sender (or the current
/// carrier of the sender name). Anything else must be rejected before enqueue.
pub(crate) fn validate_reply(
    state: &State,
    obligation: &ReplyObligation,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let sender_ok = obligation.recipient_worker_id == from
        || obligation
            .recipient_name
            .as_deref()
            .and_then(|name| name_holder(state, &obligation.project, name))
            .is_some_and(|holder| holder == from);
    let target_ok = obligation.from_worker_id == to
        || obligation
            .from_name
            .as_deref()
            .and_then(|name| name_holder(state, &obligation.project, name))
            .is_some_and(|holder| holder == to);
    if sender_ok && target_ok {
        Ok(())
    } else {
        Err(
            "REPLY_OBLIGATION_MISMATCH: only the original recipient may reply to the original sender"
                .into(),
        )
    }
}

/// Build the durable record for a message that declared a reply policy.
pub(crate) fn build_obligation(
    state: &State,
    message_id: String,
    project: String,
    from_worker_id: String,
    recipient_worker_id: String,
    recipient_name: Option<String>,
    policy: crate::proto::ReplyPolicy,
) -> ReplyObligation {
    let from_name = stable_name(state, &from_worker_id);
    ReplyObligation {
        message_id,
        project,
        from_project: None,
        from_ceo: false,
        from_worker_id,
        from_name,
        recipient_worker_id,
        recipient_name,
        policy,
        created_ms: now_ms(),
        reminded_idle_episode: false,
    }
}

#[cfg(test)]
#[path = "reply_obligation_tests.rs"]
mod reply_obligation_tests;
