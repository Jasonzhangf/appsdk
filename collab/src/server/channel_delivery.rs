//! Typed outbound platform-result ledger.
//!
//! Platform acceptance is not message consumption. These records are keyed by
//! the Collab outbound message id and never overwrite `Message.state`.
use super::state::{now_ms, Event};
use super::Server;
use crate::proto::Resp;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelDeliveryStatus {
    Accepted,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelDeliveryResult {
    pub message_id: String,
    pub channel_instance: String,
    pub status: ChannelDeliveryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub updated_ms: i64,
}

fn validate(result: &ChannelDeliveryResult) -> Result<(), String> {
    if result.message_id.trim().is_empty() {
        return Err("CHANNEL_DELIVERY_MESSAGE_REQUIRED: outbound message id is required".into());
    }
    if result.channel_instance.trim().is_empty() {
        return Err("CHANNEL_DELIVERY_CHANNEL_REQUIRED: channel instance is required".into());
    }
    match result.status {
        ChannelDeliveryStatus::Accepted => {
            if !result
                .platform_message_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
            {
                return Err(
                    "CHANNEL_DELIVERY_PLATFORM_ID_REQUIRED: accepted send requires a platform message id"
                        .into(),
                );
            }
            if result.error.is_some() {
                return Err(
                    "CHANNEL_DELIVERY_RESULT_CONFLICT: accepted send cannot carry an error".into(),
                );
            }
        }
        ChannelDeliveryStatus::Failed | ChannelDeliveryStatus::Uncertain => {
            if !result
                .error
                .as_deref()
                .is_some_and(|error| !error.trim().is_empty())
            {
                return Err(
                    "CHANNEL_DELIVERY_ERROR_REQUIRED: failed or uncertain send requires the real error"
                        .into(),
                );
            }
            if result.platform_message_id.is_some() {
                return Err(
                    "CHANNEL_DELIVERY_RESULT_CONFLICT: failed or uncertain send cannot claim a platform message id"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

/// Record one explicit platform outcome for an already committed outbound
/// message. A late query may resolve `uncertain`; an accepted result is final.
/// Recording a result never causes another platform send.
pub(crate) fn record_channel_delivery_result(
    server: &Server,
    mut result: ChannelDeliveryResult,
) -> Resp {
    if let Err(error) = validate(&result) {
        return Resp::err(error);
    }
    let mut st = server.state.lock().unwrap();
    if !st.msgs.contains_key(&result.message_id) {
        return Resp::err(format!(
            "CHANNEL_DELIVERY_MESSAGE_NOT_FOUND: {} is not a committed outbound message",
            result.message_id
        ));
    }
    if let Some(existing) = st.channel_delivery_results.get(&result.message_id) {
        if existing.status == ChannelDeliveryStatus::Accepted
            && result.status != ChannelDeliveryStatus::Accepted
        {
            return Resp::err(
                "CHANNEL_DELIVERY_ACCEPTED_FINAL: an accepted platform result cannot be downgraded",
            );
        }
        if existing == &result {
            return Resp::data(json!({ "result": existing, "deduplicated": true }));
        }
    }
    result.updated_ms = now_ms();
    if let Err(error) = server.commit_locked(
        &mut st,
        &[Event::ChannelDeliveryResultRecorded {
            result: result.clone(),
        }],
    ) {
        return Resp::err(format!("CHANNEL_DELIVERY_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({ "result": result }))
}
