//! Typed outbound platform-result ledger.
//!
//! Platform acceptance is not message consumption. These records are keyed by
//! the Collab outbound message id and never overwrite `Message.state`.
use super::channel_envelope::ChannelReplyRoute;
use super::state::{now_ms, Event, Message, State};
use super::Server;
use crate::client::adapters::qqbot::{
    delivery_result, prepare_outbound, send_prepared, OutboundAttachment, OutboundRequest,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelDeliveryStatus {
    Sending,
    Accepted,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlatformTimestamp {
    Text(String),
    Number(i64),
}

impl PlatformTimestamp {
    fn is_valid(&self) -> bool {
        !matches!(self, Self::Text(value) if value.trim().is_empty())
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_timestamp: Option<PlatformTimestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_ref_idx: Option<String>,
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
        ChannelDeliveryStatus::Sending => {
            if result.platform_message_id.is_some()
                || result.platform_timestamp.is_some()
                || result.platform_ref_idx.is_some()
                || result.error.is_some()
            {
                return Err(
                    "CHANNEL_DELIVERY_RESULT_CONFLICT: sending claim cannot carry a platform result"
                        .into(),
                );
            }
        }
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
            if !result
                .platform_timestamp
                .as_ref()
                .is_some_and(PlatformTimestamp::is_valid)
            {
                return Err(
                    "CHANNEL_DELIVERY_TIMESTAMP_REQUIRED: accepted send requires the platform timestamp"
                        .into(),
                );
            }
            if result
                .platform_ref_idx
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            {
                return Err(
                    "CHANNEL_DELIVERY_REF_INVALID: accepted send cannot carry an empty platform reference"
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
            if result.platform_timestamp.is_some() || result.platform_ref_idx.is_some() {
                return Err(
                    "CHANNEL_DELIVERY_RESULT_CONFLICT: failed or uncertain send cannot carry accepted platform fields"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

fn committed_reply_route(
    state: &State,
    message_id: &str,
) -> Result<(Message, ChannelReplyRoute), String> {
    let message = state.msgs.get(message_id).cloned().ok_or_else(|| {
        format!(
            "CHANNEL_DELIVERY_MESSAGE_NOT_FOUND: {message_id} is not a committed outbound message"
        )
    })?;
    let route = state
        .channel_reply_routes
        .get(message_id)
        .cloned()
        .ok_or_else(|| {
            format!(
                "CHANNEL_DELIVERY_ROUTE_NOT_FOUND: {message_id} has no immutable channel reply route"
            )
        })?;
    if !route.source_channel.eq_ignore_ascii_case("qqbot") {
        return Err(format!(
            "CHANNEL_DELIVERY_CHANNEL_UNSUPPORTED: {} is not a QQBot reply",
            route.source_channel
        ));
    }
    if route.channel_instance.trim().is_empty() {
        return Err(
            "CHANNEL_DELIVERY_CHANNEL_REQUIRED: the immutable reply route has no channel instance"
                .into(),
        );
    }
    if message.to != route.reply_route {
        return Err(format!(
            "CHANNEL_DELIVERY_ROUTE_MISMATCH: message target {} does not match immutable route {}",
            message.to, route.reply_route
        ));
    }
    Ok((message, route))
}

fn persist_result_locked(
    server: &Server,
    state: &mut State,
    mut result: ChannelDeliveryResult,
) -> Result<ChannelDeliveryResult, String> {
    if let Err(error) = validate(&result) {
        return Err(error);
    }
    let (_, route) = committed_reply_route(state, &result.message_id)?;
    if result.channel_instance != route.channel_instance {
        return Err(format!(
            "CHANNEL_DELIVERY_CHANNEL_MISMATCH: result channel {} does not match immutable route {}",
            result.channel_instance, route.channel_instance
        ));
    }
    if let Some(existing) = state.channel_delivery_results.get(&result.message_id) {
        if existing.channel_instance != result.channel_instance {
            return Err(
                "CHANNEL_DELIVERY_CHANNEL_MISMATCH: the existing result belongs to another channel"
                    .into(),
            );
        }
        if existing == &result {
            return Ok(existing.clone());
        }
        if existing.status != ChannelDeliveryStatus::Sending {
            return Err(format!(
                "CHANNEL_DELIVERY_RESULT_FINAL: the message already has terminal status {:?}",
                existing.status
            ));
        }
    }
    result.updated_ms = now_ms();
    if let Err(error) = server.commit_locked(
        state,
        &[Event::ChannelDeliveryResultRecorded {
            result: result.clone(),
        }],
    ) {
        return Err(format!("CHANNEL_DELIVERY_DURABILITY_FAILED: {error}"));
    }
    Ok(result)
}

fn failure_result(
    message_id: &str,
    channel_instance: &str,
    error: impl Into<String>,
) -> ChannelDeliveryResult {
    ChannelDeliveryResult {
        message_id: message_id.into(),
        channel_instance: channel_instance.into(),
        status: ChannelDeliveryStatus::Failed,
        platform_message_id: None,
        error: Some(error.into()),
        platform_timestamp: None,
        platform_ref_idx: None,
        updated_ms: 0,
    }
}

pub(crate) fn current_result(state: &State, message_id: &str) -> Option<ChannelDeliveryResult> {
    state
        .channel_delivery_results
        .get(message_id)
        .map(|result| {
            if result.status == ChannelDeliveryStatus::Sending {
                sending_projection(result)
            } else {
                result.clone()
            }
        })
}

fn sending_projection(result: &ChannelDeliveryResult) -> ChannelDeliveryResult {
    let mut projected = result.clone();
    projected.status = ChannelDeliveryStatus::Uncertain;
    projected.platform_message_id = None;
    projected.platform_timestamp = None;
    projected.platform_ref_idx = None;
    projected.error = Some(
        "CHANNEL_DELIVERY_SENDING_UNRECOVERED: the send was claimed but has no durable platform result"
            .into(),
    );
    projected
}

fn persist_final(
    server: &Server,
    result: ChannelDeliveryResult,
) -> Result<ChannelDeliveryResult, String> {
    let mut state = server.state.lock().unwrap();
    persist_result_locked(server, &mut state, result)
        .map_err(|error| format!("CHANNEL_DELIVERY_RESULT_UNCERTAIN: {error}"))
}

fn sender_label(server: &Server, state: &State, message: &Message) -> Result<String, String> {
    if let Some(meta) = state.federation.messages.get(&message.id) {
        return Ok(super::federation::display_sender(&meta.sender));
    }
    // Retained messages without a current name keep their recorded sender ID.
    let Some(route) = super::server_route_scope(server, state)? else {
        return Ok(message.from.clone());
    };
    let control =
        super::binding_store::BindingControlStore::new(server.host_paths.state_root()).load()?;
    let Some(slot) =
        control.current_for_worker(&route.project_scope_id, &route.app_scope_id, &message.from)?
    else {
        return Ok(message.from.clone());
    };
    let address = crate::proto::LocalNameAddress {
        project: super::canonical_project_key(&server.root)
            .ok_or("CHANNEL_SENDER_PROJECT_UNAVAILABLE")?,
        name: slot.identity_key.name.clone(),
    };
    match server.config.federation.as_ref() {
        Some(configuration) => Ok(super::federation::display_qualified(
            &crate::proto::QualifiedNameAddress {
                machine: configuration.machine.clone(),
                local: address,
            },
        )),
        None => crate::address::format_local_address(&address).map_err(|error| error.to_string()),
    }
}

/// Claim a committed reply once, then invoke the declared helper outside locks.
/// Existing claims return their recorded or uncertain result without resending.
pub(crate) async fn deliver_committed_reply(
    server: Arc<Server>,
    message_id: String,
) -> Result<ChannelDeliveryResult, String> {
    let (prepared, delivery) = {
        let mut state = server.state.lock().unwrap();
        let (message, route) = committed_reply_route(&state, &message_id)?;
        if let Some(existing) = state.channel_delivery_results.get(&message_id) {
            if existing.channel_instance != route.channel_instance {
                return Err(format!(
                    "CHANNEL_DELIVERY_CHANNEL_MISMATCH: existing result channel {} does not match immutable route {}",
                    existing.channel_instance, route.channel_instance
                ));
            }
            if existing.status == ChannelDeliveryStatus::Sending {
                return Ok(sending_projection(existing));
            }
            return Ok(existing.clone());
        }

        let sender_label = sender_label(&server, &state, &message)?;
        let content = format!("[{sender_label}]\n{}", message.body);
        let prepared = match prepare_outbound(
            None,
            OutboundRequest {
                collab_message_id: message.id,
                channel_instance: route.channel_instance.clone(),
                reply_route: route.reply_route.clone(),
                sender_label,
                content,
                attachments: Vec::<OutboundAttachment>::new(),
            },
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                let result = failure_result(&message_id, &route.channel_instance, error);
                let result = persist_result_locked(&server, &mut state, result)?;
                return Ok(result);
            }
        };
        let claim = ChannelDeliveryResult {
            message_id: message_id.clone(),
            channel_instance: route.channel_instance.clone(),
            status: ChannelDeliveryStatus::Sending,
            platform_message_id: None,
            error: None,
            platform_timestamp: None,
            platform_ref_idx: None,
            updated_ms: 0,
        };
        persist_result_locked(&server, &mut state, claim)
            .map_err(|error| format!("CHANNEL_DELIVERY_CLAIM_FAILED: {error}"))?;
        (prepared, server.config.qqbot.delivery.clone())
    };

    let Some(delivery) = delivery else {
        return persist_final(
            &server,
            failure_result(
                &message_id,
                &prepared.request.channel_instance,
                "QQBOT_DELIVERY_NOT_CONFIGURED: qqbot.delivery is required for outbound replies",
            ),
        );
    };

    let outcome = send_prepared(&delivery, &prepared).await;
    let result = delivery_result(&prepared, outcome);
    persist_final(&server, result)
}

#[cfg(test)]
#[path = "channel_delivery_tests.rs"]
mod channel_delivery_tests;
