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
    /// resolves to when the sender gives no explicit address. This remains the
    /// display/legacy input; the authoritative routing decision is in
    /// `resolved_target`.
    pub target_address: String,
    /// Address-owner classification persisted by the setter. Old defaults
    /// before federation have no value and keep their local legacy meaning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_target: Option<crate::address::ResolvedAddress>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn typed_default(target_address: &str) -> ConversationDefault {
        ConversationDefault {
            channel: "qqbot".into(),
            channel_instance: "qqbot@host".into(),
            conversation: "u1:session".into(),
            project_scope: "/project".into(),
            target_address: target_address.into(),
            resolved_target: None,
            revision: 1,
            updated_ms: 0,
        }
    }

    #[test]
    fn typed_machine_address_is_preserved_across_current_state_roundtrip() {
        let mut state = super::super::state::State::default();
        let default = typed_default("macbookair.appsdk.Alice");
        state
            .conversation_defaults
            .insert(default.key(), default.clone());

        let replayed: super::super::state::State =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(
            lookup_default(&replayed, "qqbot", "qqbot@host", "u1:session"),
            Some(&default),
        );
        assert_eq!(
            lookup_default(&replayed, "qqbot", "qqbot@host", "u1:session")
                .unwrap()
                .target_address,
            "macbookair.appsdk.Alice"
        );
        assert_eq!(
            lookup_default(&replayed, "qqbot", "qqbot@host", "u1:session")
                .unwrap()
                .resolved_target,
            None,
            "an untyped historical default must not gain federation meaning",
        );
    }

    #[test]
    fn typed_remote_default_is_restored_across_state_roundtrip() {
        let mut state = super::super::state::State::default();
        let mut default = typed_default("macbookair.appsdk.Alice");
        default.resolved_target = Some(crate::address::ResolvedAddress::Remote {
            machine: "macbookair".into(),
            target: crate::proto::LocalNameAddress {
                project: "appsdk".into(),
                name: "Alice".into(),
            },
        });
        state
            .conversation_defaults
            .insert(default.key(), default.clone());

        let replayed: super::super::state::State =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(
            lookup_default(&replayed, "qqbot", "qqbot@host", "u1:session"),
            Some(&default),
        );
    }

    #[test]
    fn channel_instance_isolation_keeps_typed_defaults_separate() {
        let mut state = super::super::state::State::default();
        let first = ConversationDefault {
            channel_instance: "qqbot@a".into(),
            ..typed_default("macbookair.appsdk.Alice")
        };
        let second = ConversationDefault {
            channel_instance: "qqbot@b".into(),
            ..typed_default("coder1.appsdk.Alice")
        };
        state.conversation_defaults.insert(first.key(), first);
        state.conversation_defaults.insert(second.key(), second);

        assert_eq!(
            lookup_default(&state, "qqbot", "qqbot@a", "u1:session")
                .unwrap()
                .target_address,
            "macbookair.appsdk.Alice"
        );
        assert_eq!(
            lookup_default(&state, "qqbot", "qqbot@b", "u1:session")
                .unwrap()
                .target_address,
            "coder1.appsdk.Alice"
        );
    }
}
