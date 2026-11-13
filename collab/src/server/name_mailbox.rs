//! Name handoff uses the existing project/app runtime mailbox, never a copy.
use super::name_state::NameOccupancy;
use super::state::{Event, Message, NotificationDeliveryFailure, State};
use serde::{Deserialize, Serialize};

/// Original typed name address. This does not follow worker display-name changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailboxNameAddress {
    pub project_scope: String,
    pub name: String,
}

pub(crate) fn current_name_allows_message(
    state: &State,
    control: &super::binding_slot::BindingControlSet,
    scope: &crate::scope::RouteScope,
    worker_id: &str,
    authorized_binding: &super::RuntimeBinding,
    message: &Message,
) -> Result<bool, String> {
    let Some(address) = state.mailbox_name_addresses.get(&message.id) else {
        return Ok(true);
    };
    if address.project_scope != scope.project_scope_id.as_str() {
        return Ok(false);
    }
    Ok(control
        .current_for_name(&scope.project_scope_id, &scope.app_scope_id, &address.name)?
        .and_then(|slot| slot.active_binding())
        .is_some_and(|current| {
            current.binding.agent_id.as_str() == worker_id
                && &current.binding == authorized_binding
        }))
}

/// Current mailbox mutation input. Native acceptance is delivery evidence,
/// not a receive receipt. The baseline grants one wake to the new execution carrier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxHandoff {
    pub project_scope: String,
    pub name: String,
    pub occupancy_revision: u64,
    pub session_id: String,
    pub message_id: String,
    pub previous_recipient: String,
    pub recipient: String,
    pub wake_attempt_count: u32,
    pub previous_subscription: Option<String>,
    pub accepted_ms: Option<i64>,
    pub evidence: Option<serde_json::Value>,
    pub failure: Option<NotificationDeliveryFailure>,
}

pub(crate) fn handoff_events(
    state: &State,
    project: &str,
    next: &NameOccupancy,
    rename_from: &[String],
    previous: Option<&NameOccupancy>,
) -> Vec<Event> {
    if rename_from.is_empty() && previous.is_some_and(|previous| {
        previous.worker_id == next.worker_id && previous.session_id == next.session_id
    }) {
        return Vec::new();
    }
    // The caller holds this project/app runtime's state lock. A different
    // project's name record cannot select a recipient in this mailbox.
    let mut messages: Vec<_> = state
        .msgs
        .values()
        .filter(|message| {
            matches!(message.state.as_str(), "pending" | "delivered")
                && match state.mailbox_name_addresses.get(&message.id) {
                    Some(address) => {
                        address.project_scope == project
                            && address.name == next.name
                    }
                    // Worker-addressed mail follows only a known current holder.
                    None => previous.is_some_and(|holder| message.to == holder.worker_id),
                }
                && !(previous.is_none()
                    && message.to == next.worker_id
                    && !state.mailbox_name_addresses.get(&message.id).is_some_and(|address|
                        address.name != next.name && rename_from.contains(&address.name)))
        })
        .collect();
    messages.sort_by(|a, b| a.id.cmp(&b.id));
    messages
        .into_iter()
        .map(|message| Event::MailboxReassigned {
            handoff: MailboxHandoff {
                project_scope: project.into(),
                name: next.name.clone(),
                occupancy_revision: next.occupancy_revision,
                session_id: next.session_id.clone(),
                message_id: message.id.clone(),
                previous_recipient: message.to.clone(),
                recipient: next.worker_id.clone(),
                wake_attempt_count: message.wake_attempt_count,
                previous_subscription: state.wake_bindings.get(&message.id).cloned(),
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
        })
        .collect()
}

impl State {
    /// Whether `worker_id` is the current mailbox recipient of `message`.
    /// Typed name mail is bound to the stable name: the caller must be the
    /// name's current holder, so a reused actor that now holds another name
    /// cannot read, consume or ack the released source name's messages. Raw
    /// worker-addressed mail keeps its direct-send semantics. The occupancy
    /// read shares the caller's state lock with the handoff commit, so the
    /// eligibility check and the consumption commit stay serialized.
    pub(crate) fn message_receivable_by(&self, worker_id: &str, message: &Message) -> bool {
        if message.to != worker_id {
            return false;
        }
        let Some(address) = self.mailbox_name_addresses.get(&message.id) else {
            return true;
        };
        self.name_occupancy(&address.project_scope, &address.name)
            .is_some_and(|holder| holder.worker_id == worker_id)
    }

    pub(crate) fn apply_mailbox_handoff(&mut self, handoff: &MailboxHandoff) {
        if let Some(message) = self.msgs.get_mut(&handoff.message_id) {
            if message.to == handoff.previous_recipient
                && matches!(message.state.as_str(), "pending" | "delivered")
            {
                message.to = handoff.recipient.clone();
                self.mailbox_handoff_wakes.insert(message.id.clone(), message.wake_attempt_count);
                self.wake_bindings.insert(
                    message.id.clone(),
                    super::mailbox::default_direct_message_id(&handoff.recipient),
                );
                if self.target_sessions.contains_key(&message.id) {
                    self.target_sessions
                        .insert(message.id.clone(), handoff.session_id.clone());
                }
            }
        }
    }

    pub(crate) fn handoff_wake_pending(&self, message: &Message) -> bool {
        matches!(message.state.as_str(), "pending" | "delivered")
            && self.mailbox_handoff_wakes.get(&message.id) == Some(&message.wake_attempt_count)
    }
}
