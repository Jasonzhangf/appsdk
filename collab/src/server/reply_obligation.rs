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
pub(crate) fn stable_name(
    control: &super::binding_slot::BindingControlSet,
    project: &crate::scope::ProjectScopeId,
    app: &crate::identity::AppServerId,
    worker_id: &str,
) -> Result<Option<String>, String> {
    Ok(control
        .current_for_worker(project, app, worker_id)?
        .map(|slot| slot.identity_key.name.clone()))
}

fn name_holder<'a>(
    control: &'a super::binding_slot::BindingControlSet,
    project_scope: &str,
    app: &crate::identity::AppServerId,
    name: &str,
) -> Result<Option<&'a str>, String> {
    let project =
        crate::scope::ProjectScopeId::new(project_scope).map_err(|error| error.to_string())?;
    Ok(control
        .current_for_name(&project, app, name)?
        .and_then(|slot| slot.active_binding())
        .map(|current| current.binding.agent_id.as_str()))
}

/// Allow the current carrier of an open obligation's recipient name to read
/// the original message. The check uses only the typed obligation and the
/// caller's exact current project/app binding; read/ack state is irrelevant.
pub(crate) fn current_obligation_recipient_can_read(
    control: &super::binding_slot::BindingControlSet,
    project_scope: &crate::scope::ProjectScopeId,
    app: &crate::identity::AppServerId,
    message_id: &str,
    obligation: Option<&ReplyObligation>,
    caller_worker_id: &str,
) -> Result<bool, String> {
    let Some(obligation) = obligation else {
        return Ok(false);
    };
    if obligation.message_id != message_id || obligation.project != project_scope.as_str() {
        return Ok(false);
    }
    let Some(recipient_name) = obligation.recipient_name.as_deref() else {
        return Ok(false);
    };
    Ok(name_holder(control, &obligation.project, app, recipient_name)? == Some(caller_worker_id))
}

/// Verify that `from` is the original recipient (or the current carrier of the
/// recipient name) replying to `to`, the original sender (or the current
/// carrier of the sender name). Anything else must be rejected before enqueue.
pub(crate) fn validate_reply(
    control: &super::binding_slot::BindingControlSet,
    app: &crate::identity::AppServerId,
    obligation: &ReplyObligation,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let sender_ok = obligation.recipient_worker_id == from
        || match obligation.recipient_name.as_deref() {
            Some(name) => name_holder(control, &obligation.project, app, name)? == Some(from),
            None => false,
        };
    let target_ok = obligation.from_worker_id == to
        || match obligation.from_name.as_deref() {
            Some(name) => {
                name_holder(
                    control,
                    obligation
                        .from_project
                        .as_deref()
                        .unwrap_or(&obligation.project),
                    app,
                    name,
                )? == Some(to)
            }
            None => false,
        };
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
    from_name: Option<String>,
    message_id: String,
    project: String,
    from_worker_id: String,
    recipient_worker_id: String,
    recipient_name: Option<String>,
    policy: crate::proto::ReplyPolicy,
) -> ReplyObligation {
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
