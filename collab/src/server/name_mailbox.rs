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

/// Immutable handoff history. Native acceptance is historical delivery evidence,
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

pub(crate) fn handoff_events(state: &State, project: &str, next: &NameOccupancy) -> Vec<Event> {
    let previous = state.name_occupancy(project, &next.name);
    if previous.is_some_and(|previous| {
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
                    Some(address) => address.project_scope == project && address.name == next.name,
                    // Old journals cannot recover historical names. Only the
                    // current previous holder is a valid legacy selector.
                    None => previous.is_some_and(|holder| message.to == holder.worker_id),
                }
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
    pub(crate) fn apply_mailbox_handoff(&mut self, handoff: &MailboxHandoff) {
        if self.mailbox_handoffs.iter().any(|existing| {
            existing.project_scope == handoff.project_scope
                && existing.name == handoff.name
                && existing.occupancy_revision == handoff.occupancy_revision
                && existing.message_id == handoff.message_id
        }) {
            return;
        }
        self.mailbox_handoffs.push(handoff.clone());
        if let Some(message) = self.msgs.get_mut(&handoff.message_id) {
            if message.to == handoff.previous_recipient
                && matches!(message.state.as_str(), "pending" | "delivered")
            {
                message.to = handoff.recipient.clone();
                self.wake_bindings.insert(
                    message.id.clone(),
                    super::mailbox::default_direct_message_id(&handoff.recipient),
                );
            }
        }
    }

    pub(crate) fn handoff_wake_pending(&self, message: &Message) -> bool {
        matches!(message.state.as_str(), "pending" | "delivered")
            && self
                .mailbox_handoffs
                .iter()
                .rev()
                .find(|handoff| handoff.message_id == message.id)
                .is_some_and(|handoff| {
                    handoff.recipient == message.to
                        && handoff.wake_attempt_count == message.wake_attempt_count
                })
    }
}
