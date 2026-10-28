//! Per-external-conversation default delivery target.
//!
//! A "default" only decides where a channel-inbound message without an explicit
//! address is routed. It never grants identity, never limits a legitimate
//! agent from replying to an original message, and never changes the reply
//! route of an already-received message. The key is channel-owned: each QQ
//! conversation is isolated by `<channel>:<conversation>`, so user A's switch
//! cannot affect user B.
use serde::{Deserialize, Serialize};

/// One persistent default delivery target for a single external conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationDefault {
    /// Channel instance, e.g. `qqbot`.
    pub channel: String,
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
        conversation_default_key(&self.channel, &self.conversation)
    }
}

/// Build the durable map key from the channel-owned conversation identity.
pub fn conversation_default_key(channel: &str, conversation: &str) -> String {
    format!("{}:{}", channel, conversation)
}

/// Resolve the default delivery target for one external conversation.
pub fn lookup_default<'a>(
    state: &'a super::state::State,
    channel: &str,
    conversation: &str,
) -> Option<&'a ConversationDefault> {
    state
        .conversation_defaults
        .get(&conversation_default_key(channel, conversation))
}
