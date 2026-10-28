//! Per-external-conversation default delivery target.
//!
//! A "default" only decides where a channel-inbound message without an explicit
//! address is routed. It never grants identity, never limits a legitimate
//! agent from replying to an original message, and never changes the reply
//! route of an already-received message. The key is channel-owned: each
//! conversation is isolated by `<channel>:<channel-instance>:<conversation>`,
//! so two QQBot instances or two external users never share one default.
use serde::{Deserialize, Serialize};

/// One persistent default delivery target for a single external conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationDefault {
    /// Channel family, e.g. `qqbot`.
    pub channel: String,
    /// Distinct platform connection, e.g. `qqbot@host`. Two instances of the
    /// same channel keep independent defaults even for one conversation key.
    #[serde(default)]
    pub channel_instance: String,
    /// Stable external conversation key, e.g. `<user-id>:<session-id>`.
    pub conversation: String,
    /// Project whose directory supplied the resolved target. Project reset
    /// removes only defaults owned by that exact scope.
    #[serde(default)]
    pub project_scope: String,
    /// Project-qualified or stable-name target address the inbound message
    /// resolves to when the sender gives no explicit address.
    pub target_address: String,
    /// Monotonic revision, advanced on every accepted switch.
    pub revision: u64,
    pub updated_ms: i64,
}

impl ConversationDefault {
    /// Stable map key for this conversation's default.
    pub fn key(&self) -> String {
        conversation_default_key(&self.channel, &self.channel_instance, &self.conversation)
    }
}

/// Build the durable map key from the channel-owned identity. The channel
/// instance is mandatory so two QQBot connections cannot share a default.
pub fn conversation_default_key(
    channel: &str,
    channel_instance: &str,
    conversation: &str,
) -> String {
    format!("{}:{}:{}", channel, channel_instance, conversation)
}

/// Resolve the default delivery target for one channel instance and external
/// conversation.
pub fn lookup_default<'a>(
    state: &'a super::state::State,
    channel: &str,
    channel_instance: &str,
    conversation: &str,
) -> Option<&'a ConversationDefault> {
    state.conversation_defaults.get(&conversation_default_key(
        channel,
        channel_instance,
        conversation,
    ))
}
