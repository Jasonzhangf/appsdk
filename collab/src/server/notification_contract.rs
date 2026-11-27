//! Current-state boundary for daemon notifications.

/// Exact destination selected for the latest attempt of a current message.
/// This is delivery evidence; identity selection remains with the map owner.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectedDeliveryBinding {
    pub(crate) identity_key: super::binding_slot::IdentityBindingKey,
    pub(crate) binding: super::global_state::RuntimeBinding,
    pub(crate) transport: crate::proto::SelectedTransport,
    #[serde(default)]
    pub(crate) binding_generation: u64,
}

impl SelectedDeliveryBinding {
    pub(crate) fn from_slot(slot: &super::binding_slot::IdentityBindingSlot) -> Option<Self> {
        let current = slot.active_binding()?;
        Some(Self {
            identity_key: slot.identity_key.clone(),
            binding: current.binding.clone(),
            transport: current.transport.clone()?,
            binding_generation: slot.generation(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitReceipt {
    pub sequence: u64,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandOutcome {
    pub receipt: CommitReceipt,
    pub operation_id: String,
    pub outcome: serde_json::Value,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationError {
    Persistence(String),
    DurabilityUnknown(String),
    InvalidState(String),
    InvalidCommand(String),
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Persistence(error) => write!(formatter, "CURRENT_STATE_WRITE_FAILED: {error}"),
            Self::DurabilityUnknown(error) => write!(
                formatter,
                "CURRENT_STATE_DURABILITY_UNKNOWN: published=true; {error}"
            ),
            Self::InvalidState(error) => write!(formatter, "CURRENT_STATE_INVALID: {error}"),
            Self::InvalidCommand(error) => write!(formatter, "invalid command: {error}"),
        }
    }
}
impl std::error::Error for MutationError {}
