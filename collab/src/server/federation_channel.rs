//! Federated QQ channel owner adapter.

use super::channel_envelope::{ChannelEnvelope, ChannelReplyRoute};
use super::federation::{self, FederatedOutboundRecord};
use super::*;
use crate::federation::auth::AuthenticatedMachine;
use crate::identity::MessageId;
use crate::proto::{
    FederatedChannelRef, FederationActor, FederationDestination, FederationEndpoint, FederationPut,
    FederationReq, FederationResponse, LocalNameAddress, MessageRef, OriginMessageRef,
    QualifiedNameAddress,
};
use std::time::Duration;

const REQUEST_DEADLINE: Duration = Duration::from_secs(15);

/// Prepare the origin outbound record without writing any state. The caller
/// holds the trusted envelope and the already-resolved remote destination.
pub(super) fn prepare_inbound_record(
    local_machine: &str,
    scope: &ProjectScopeId,
    app: &AppServerId,
    subject: String,
    body: String,
    destination: QualifiedNameAddress,
    now: i64,
) -> Result<FederatedOutboundRecord, String> {
    // The platform index atomically binds the first candidate to its event.
    let inbound_id = gen_msg_id();
    let operation_id = crate::identity::OperationId::new(inbound_id.clone())
        .map_err(|error| format!("CHANNEL_OPERATION_ID_INVALID: {error}"))?;
    let channel = FederatedChannelRef {
        owner_machine: local_machine.to_owned(),
        project_scope: scope.clone(),
        app_scope_id: app.clone(),
        inbound_id,
    };
    federation::prepare_outbound(
        federation::AddressSendRequest {
            sender: FederationActor::ExternalChannel(channel.clone()),
            target: FederationEndpoint::Named(destination),
            mtype: "message".to_owned(),
            subject,
            body,
            delivery: "immediate".to_owned(),
            operation_id: Some(operation_id),
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            target_session: None,
            channel: Some(channel),
            now_ms: now,
        },
        &local_machine.to_owned(),
    )
    .map_err(|error| error.to_string())
}

/// One validated federated channel reply. Pure, no IO or lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelReplyIntent {
    pub origin: OriginMessageRef,
    pub in_reply_to: MessageRef,
    pub original_origin: OriginMessageRef,
    pub mtype: String,
    pub subject: String,
    pub body: String,
}

/// Validate a federated channel reply against the original outbound intent.
/// Pure: no locks, no IO, no state change. Unknown-original lookup recovery
/// is the caller's responsibility before calling this function.
pub fn validate_channel_reply(
    connection: &AuthenticatedMachine,
    request: &FederationPut,
    record: &FederatedOutboundRecord,
) -> Result<ChannelReplyIntent, federation::FederationError> {
    let FederationDestination::ChannelOwner(channel) = &request.target else {
        return Err(federation::FederationError::new(
            "FEDERATION_TARGET_INVALID",
            "channel-owner acceptance requires a channel-owner destination",
        ));
    };
    if !connection
        .source
        .eq_ignore_ascii_case(record.target.machine())
        || !channel
            .owner_machine
            .eq_ignore_ascii_case(&connection.target)
        || record.sender != FederationActor::ExternalChannel(channel.clone())
    {
        return Err(federation::FederationError::new(
            "MACHINE_IDENTITY_MISMATCH",
            format!(
                "connection source {} is not the original outbound target {}",
                connection.source,
                record.target.machine()
            ),
        ));
    }
    if request.origin.machine != connection.source {
        return Err(federation::FederationError::new(
            "FEDERATION_SOURCE_MISMATCH",
            "reply origin machine does not match the authenticated connection source",
        ));
    }
    let FederationActor::Peer(sender) = &request.sender else {
        return Err(federation::FederationError::new(
            "FEDERATION_PARTICIPANT_MISMATCH",
            "a federated channel reply must come from the original named recipient",
        ));
    };
    let FederationEndpoint::Named(original_target) = &record.target else {
        return Err(federation::FederationError::new(
            "FEDERATION_CHANNEL_MISMATCH",
            "original channel outbound did not target a named recipient",
        ));
    };
    if !federation::same_endpoint(
        &FederationEndpoint::Named(sender.address.clone()),
        &FederationEndpoint::Named(original_target.clone()),
    ) {
        return Err(federation::FederationError::new(
            "FEDERATION_PARTICIPANT_MISMATCH",
            "reply sender is not the original named recipient",
        ));
    }
    let accepted = record.accepted.as_ref().ok_or_else(|| {
        federation::FederationError::new(
            "FEDERATION_REPLY_ORIGIN_UNCONFIRMED",
            "original outbound is not yet accepted on the target",
        )
    })?;
    if sender.project_scope != accepted.project_scope
        || sender.app_scope_id != accepted.app_scope_id
    {
        return Err(federation::FederationError::new(
            "FEDERATION_SCOPE_MISMATCH",
            "reply sender scope does not match the accepted remote mailbox",
        ));
    }
    let Some(in_reply_to) = request.in_reply_to.as_ref() else {
        return Err(federation::FederationError::new(
            "FEDERATION_REPLY_REFERENCE_REQUIRED",
            "a federated channel reply must carry in_reply_to",
        ));
    };
    if in_reply_to != accepted {
        return Err(federation::FederationError::new(
            "FEDERATION_REPLY_REFERENCE_MISMATCH",
            "in_reply_to does not match the original accepted target reference",
        ));
    }
    if request.channel.as_ref() != Some(channel) || request.channel != record.channel {
        return Err(federation::FederationError::new(
            "FEDERATION_CHANNEL_MISMATCH",
            "reply channel reference does not match the original outbound",
        ));
    }
    let Some(original_origin) = request.original_origin.as_ref() else {
        return Err(federation::FederationError::new(
            "FEDERATION_REPLY_ORIGIN_REQUIRED",
            "a federated channel reply must carry original_origin",
        ));
    };
    if original_origin != &record.origin {
        return Err(federation::FederationError::new(
            "FEDERATION_REPLY_ORIGIN_MISMATCH",
            "original_origin does not match the original outbound",
        ));
    }
    if request.subject.trim().is_empty() {
        return Err(federation::FederationError::new(
            "MESSAGE_SUBJECT_REQUIRED",
            "a federated channel reply requires a subject",
        ));
    }
    if request.delivery != "immediate"
        || request.target_session.is_some()
        || request.reply_policy.is_some()
    {
        return Err(federation::FederationError::new(
            "FEDERATION_CHANNEL_DELIVERY_UNSUPPORTED", "channel replies require immediate delivery without a target session or reply obligation"));
    }
    Ok(ChannelReplyIntent {
        origin: request.origin.clone(),
        in_reply_to: in_reply_to.clone(),
        original_origin: original_origin.clone(),
        mtype: request.mtype.clone(),
        subject: request.subject.clone(),
        body: request.body.clone(),
    })
}

/// Result of building the reply commit batch.
pub struct ReplyBatch {
    pub reference: MessageRef,
    pub events: Vec<Event>,
}

/// Build the reply events for one accepted channel reply. The reply mailbox
/// row, the federation acceptance metadata, and the immutable reply route are
/// one current-state batch. The channel reply has no local worker subscription.
pub fn build_reply_events(
    intent: &ChannelReplyIntent,
    sender: &FederationActor,
    sender_display: &str,
    local_machine: &str,
    channel: &FederatedChannelRef,
    envelope: &ChannelEnvelope,
    now: i64,
) -> Result<ReplyBatch, federation::FederationError> {
    let mid = gen_msg_id();
    let msg_id = MessageId::new(mid.clone()).map_err(|error| {
        federation::FederationError::new("MESSAGE_ID_INVALID", error.to_string())
    })?;
    let reference = MessageRef {
        machine: local_machine.to_owned(),
        project_scope: channel.project_scope.clone(),
        app_scope_id: channel.app_scope_id.clone(),
        msg_id,
    };
    let meta = federation::FederatedMessageMeta {
        origin: intent.origin.clone(),
        sender: sender.clone(),
        target: FederationDestination::ChannelOwner(channel.clone()),
        recipient: FederationActor::ExternalChannel(channel.clone()),
        reference: reference.clone(),
        mtype: intent.mtype.clone(),
        subject: intent.subject.clone(),
        delivery: "immediate".to_owned(),
        in_reply_to: Some(intent.in_reply_to.clone()),
        original_origin: Some(intent.original_origin.clone()),
        reply_policy: None,
        channel: Some(channel.clone()),
        accepted_ms: now,
    };
    let msg = Message {
        id: mid.clone(),
        from: sender_display.to_owned(),
        to: envelope.reply_route.clone(),
        mtype: intent.mtype.clone(),
        subject: Some(intent.subject.clone()),
        body: intent.body.clone(),
        in_reply_to: Some(intent.in_reply_to.msg_id.to_string()),
        created_ms: now,
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    let events = vec![
        Event::Sent { msg },
        Event::DeliveryMode {
            msg_id: mid.clone(),
            mode: "immediate".into(),
            source_thread_id: None,
        },
        Event::FederationAccepted {
            message_id: mid.clone(),
            meta,
        },
        Event::ChannelReplyRouteBound {
            message_id: mid.clone(),
            route: ChannelReplyRoute::from_envelope(envelope, envelope.platform_message_id.clone()),
        },
    ];
    Ok(ReplyBatch { reference, events })
}

/// Host-local inbound: one trusted envelope plus an already-resolved remote
/// target. No local mailbox row is created.
pub(super) async fn inbound(
    manager: Arc<ProjectRuntimeManager>,
    context: Option<ProjectContext>,
    req: Req,
    destination: QualifiedNameAddress,
) -> (Arc<Server>, Resp) {
    let host = manager.host.clone();
    let context = match context {
        Some(context) => context,
        None => return (host, Resp::err("PROJECT_CONTEXT_REQUIRED")),
    };
    let Req::ChannelInbound {
        from: _,
        to: _,
        subject,
        body,
        envelope,
    } = req
    else {
        return (host, Resp::err("FEDERATION_REQUEST_INVALID"));
    };
    let server = match manager.select_runtime(&context) {
        Ok(server) => server,
        Err(error) => return (host, Resp::err(error)),
    };
    let configuration = match manager.host.config.federation.clone() {
        Some(configuration) => configuration,
        None => {
            return (
                server,
                Resp::err("FEDERATION_NOT_CONFIGURED: remote channel target requires federation"),
            )
        }
    };
    if let Err(error) = validate_channel_envelope_for_inbound(&envelope) {
        return (server, Resp::err(error));
    }
    if !envelope.attachments.is_empty() {
        return (
            server,
            Resp::err("CHANNEL_ATTACHMENT_LOCAL_ONLY: remote channel sends support text only"),
        );
    }
    let prepare_server = server.clone();
    let prepare_manager = manager.clone();
    let machine = configuration.machine.clone();
    let prepared = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let _accept = prepare_manager.federation_accept_gate.lock().unwrap();
        let mut state = prepare_server.state.lock().unwrap();
        if state.admission_frozen() {
            return Err("PROJECT_RESET_PENDING".into());
        }
        let key = channel_envelope::platform_event_key(
            &envelope.channel_instance,
            &envelope.external_conversation,
            &envelope.platform_message_id,
        );
        if let Some(existing) = state.platform_event_index.get(&key) {
            return Ok(existing.clone());
        }
        let record = prepare_inbound_record(
            &machine,
            &context.project_scope,
            &context.app_scope_id,
            subject,
            body,
            destination,
            now_ms(),
        )?;
        let channel = record
            .channel
            .as_ref()
            .ok_or("FEDERATION_CHANNEL_MISMATCH")?;
        let target = channel_envelope::PlatformEventTarget::FederatedOrigin(record.origin.clone());
        prepare_server
            .commit_locked_checked(
                &mut state,
                &[
                    Event::FederationOutboundPrepared {
                        record: record.clone(),
                    },
                    Event::FederatedChannelInboundRecorded {
                        origin: record.origin.clone(),
                        inbound_id: channel.inbound_id.clone(),
                        idempotency_key: key,
                        envelope,
                    },
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(target)
    })
    .await;
    let origin = match prepared {
        Ok(Ok(channel_envelope::PlatformEventTarget::LocalMessage(id))) => {
            return (
                server,
                Resp::data(json!({"msg_id": id, "deduplicated": true})),
            )
        }
        Ok(Ok(channel_envelope::PlatformEventTarget::FederatedOrigin(origin))) => origin,
        Ok(Err(error)) => return (server, Resp::err(error)),
        Err(error) => {
            return (
                server,
                Resp::err(format!("CHANNEL_HANDLER_FAILED: {error}")),
            )
        }
    };
    let candidate = server
        .state
        .lock()
        .unwrap()
        .federation
        .outbound_record(&origin)
        .cloned();
    let Some(candidate) = candidate else {
        return (server, Resp::err("FEDERATION_CHANNEL_ORIGIN_MISSING"));
    };
    let response =
        super::federation_runtime::submit_record(manager, server.clone(), configuration, candidate)
            .await;
    (server, response)
}

async fn deliver_reply_response(
    owner: Arc<Server>,
    response: FederationResponse,
    requester: FederationActor,
) -> FederationResponse {
    let FederationResponse::Message { receipt } = response else {
        return response;
    };
    let reference = receipt.reference;
    let result =
        channel_delivery::deliver_committed_reply(owner.clone(), reference.msg_id.to_string())
            .await;
    let mut response =
        super::federation_receipt_response(&owner.state.lock().unwrap(), &reference, &requester);
    if let Err(error) = result {
        if let FederationResponse::Message { receipt } = &mut response {
            receipt.channel_delivery_error = Some(error);
        }
    }
    response
}

/// Accept a channel reply at its original owner and deliver its committed row.
pub(super) async fn accept_reply(
    manager: Arc<ProjectRuntimeManager>,
    connection: AuthenticatedMachine,
    request: FederationPut,
) -> FederationResponse {
    let host = manager.host.clone();
    let Some(configuration) = host.config.federation.clone() else {
        return federation_error_response(federation::FederationError::new(
            "FEDERATION_NOT_CONFIGURED",
            "remote channel targets require federation configuration",
        ));
    };
    match super::federation_runtime::origin_owner(&manager, &request.origin) {
        Ok(Some(owner)) => {
            let response = super::lookup_federated_message(
                &owner,
                &configuration.machine,
                &connection,
                &request.origin,
                &request.sender,
                Some(&request),
            );
            return deliver_reply_response(owner, response, request.sender).await;
        }
        Ok(None) => {}
        Err(error) => {
            return federation_error_response(federation::FederationError::new(
                "FEDERATION_CHANNEL_FAILED",
                error,
            ))
        }
    }
    let Some(original_origin) = request.original_origin.as_ref() else {
        return federation_error_response(federation::FederationError::new(
            "FEDERATION_REPLY_ORIGIN_REQUIRED",
            "a federated channel reply must carry original_origin",
        ));
    };
    let scope = original_origin.project_scope.clone();
    let app = original_origin.app_scope_id.clone();
    let owner = match super::federation_runtime::runtime_for_reference(&manager, &scope, &app) {
        Ok(owner) => owner,
        Err(error) => {
            return federation_error_response(federation::FederationError::new(
                "FEDERATION_REFERENCE_SCOPE_UNKNOWN",
                error,
            ))
        }
    };
    let record = {
        let state = owner.state.lock().unwrap();
        state.federation.outbound_record(original_origin).cloned()
    };
    let Some(mut record) = record else {
        return federation_error_response(federation::FederationError::new(
            "FEDERATION_REPLY_ORIGIN_UNKNOWN",
            "no original channel outbound on this machine for original_origin",
        ));
    };
    if record.status != FederatedOutboundRecord::STATUS_ACCEPTED {
        // Unknown original acceptance: confirm with the target before any
        // acceptance. A comparison-only Lookup never accepts a new message.
        let response = crate::federation::FederationClient::call(
            &configuration,
            record.target.machine(),
            &FederationReq::Lookup {
                origin: record.origin.clone(),
                requester: record.sender.clone(),
                compare: None,
            },
            REQUEST_DEADLINE,
        )
        .await;
        let response = response.map(|receipt| {
            if let Some(error) = receipt.cleanup_error {
                eprintln!(
                    "FEDERATION_CONNECTION_CLEANUP_FAILED: channel reply verification: {error}"
                );
            }
            receipt.response
        });
        match response {
            Ok(response @ FederationResponse::Message { .. }) => {
                let outcome = federation::outcome_from_response(&record, &response);
                if !matches!(outcome, federation::FederationSendOutcome::Accepted { .. }) {
                    return federation_error_response(federation::FederationError::new(
                        "FEDERATION_RESPONSE_MISMATCH",
                        "original target acceptance does not match the outbound intent",
                    ));
                }
                let mut state = owner.state.lock().unwrap();
                let Some(current) = state.federation.outbound_record(&record.origin).cloned()
                else {
                    return federation_error_response(federation::FederationError::new(
                        "FEDERATION_REPLY_ORIGIN_UNKNOWN",
                        "original outbound was removed during lookup",
                    ));
                };
                record = if current.accepted.is_some() {
                    current
                } else {
                    federation::record_with_outcome(&current, &outcome, now_ms())
                };
                if let Err(error) = owner.commit_locked_checked(
                    &mut state,
                    &[Event::FederationOutboundResulted {
                        record: record.clone(),
                    }],
                ) {
                    return federation_error_response(federation::FederationError::new(
                        "FEDERATION_ACCEPT_DURABILITY_FAILED",
                        error.to_string(),
                    ));
                }
            }
            Ok(_) | Err(_) => {
                return federation_error_response(federation::FederationError::new(
                    "FEDERATION_REPLY_ORIGIN_UNCONFIRMED",
                    "the original outbound acceptance could not be confirmed",
                ))
            }
        }
    }
    let delivery_owner = owner.clone();
    let requester = request.sender.clone();
    let committed = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let _accept = manager.federation_accept_gate.lock().unwrap();
        if let Some(existing) = super::federation_runtime::origin_owner(&manager, &request.origin)?
        {
            return Ok(super::lookup_federated_message(
                &existing,
                &configuration.machine,
                &connection,
                &request.origin,
                &request.sender,
                Some(&request),
            ));
        }
        let mut state = owner.state.lock().unwrap();
        if state.admission_frozen() {
            return Err("PROJECT_RESET_PENDING".into());
        }
        let current = state
            .federation
            .outbound_record(&record.origin)
            .ok_or("FEDERATION_REPLY_ORIGIN_UNKNOWN")?;
        let intent = validate_channel_reply(&connection, &request, current)
            .map_err(|error| error.to_string())?;
        let channel = request
            .channel
            .as_ref()
            .ok_or("FEDERATION_CHANNEL_MISMATCH")?;
        let envelope = state
            .channel_envelopes
            .get(&channel.inbound_id)
            .ok_or("CHANNEL_ENVELOPE_MISSING: original envelope is no longer retained")?;
        validate_channel_envelope_for_inbound(envelope)?;
        let batch = build_reply_events(
            &intent,
            &request.sender,
            &federation::display_sender(&request.sender),
            &configuration.machine,
            channel,
            envelope,
            now_ms(),
        )
        .map_err(|error| error.to_string())?;
        owner
            .commit_locked_checked(&mut state, &batch.events)
            .map_err(|error| error.to_string())?;
        Ok(super::federation_receipt_response(
            &state,
            &batch.reference,
            &request.sender,
        ))
    })
    .await;
    let response = match committed {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => federation_error_response(federation::FederationError::new(
            "FEDERATION_CHANNEL_FAILED",
            error,
        )),
        Err(error) => federation_error_response(federation::FederationError::new(
            "FEDERATION_CHANNEL_FAILED",
            error.to_string(),
        )),
    };
    deliver_reply_response(delivery_owner, response, requester).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_origin_prepares_matching_actor_and_channel_facts() {
        let record = prepare_inbound_record(
            "studio",
            &ProjectScopeId::new("/projects/channel").unwrap(),
            &AppServerId::new("appserver-channel").unwrap(),
            "subject".into(),
            "body".into(),
            QualifiedNameAddress {
                machine: "air".into(),
                local: LocalNameAddress {
                    project: "remote".into(),
                    name: "Bob".into(),
                },
            },
            1,
        )
        .unwrap();
        let FederationActor::ExternalChannel(channel) = &record.sender else {
            panic!("external actor required");
        };
        assert_eq!(record.channel.as_ref(), Some(channel));
    }

    #[test]
    fn channel_reply_validates_against_the_accepted_remote_scope() {
        let source_scope = ProjectScopeId::new("/projects/channel").unwrap();
        let source_app = AppServerId::new("appserver-channel").unwrap();
        let remote_scope = ProjectScopeId::new("/different/remote").unwrap();
        let remote_app = AppServerId::new("appserver-remote").unwrap();
        let channel = FederatedChannelRef {
            owner_machine: "studio".into(),
            project_scope: source_scope,
            app_scope_id: source_app,
            inbound_id: "owned-inbound".into(),
        };
        let target = QualifiedNameAddress {
            machine: "air".into(),
            local: LocalNameAddress {
                project: "remote".into(),
                name: "Bob".into(),
            },
        };
        let mut record = federation::prepare_outbound(
            federation::AddressSendRequest {
                sender: FederationActor::ExternalChannel(channel.clone()),
                target: FederationEndpoint::Named(target.clone()),
                mtype: "message".into(),
                subject: "subject".into(),
                body: "body".into(),
                delivery: "immediate".into(),
                operation_id: None,
                in_reply_to: None,
                original_origin: None,
                reply_policy: None,
                target_session: None,
                channel: Some(channel.clone()),
                now_ms: 1,
            },
            &"studio".into(),
        )
        .unwrap();
        let accepted = MessageRef {
            machine: "air".into(),
            project_scope: remote_scope.clone(),
            app_scope_id: remote_app.clone(),
            msg_id: MessageId::new("remote-message").unwrap(),
        };
        record.accepted = Some(accepted.clone());
        record.status = FederatedOutboundRecord::STATUS_ACCEPTED.into();
        let sender = FederationActor::Peer(crate::proto::FederationPrincipal {
            address: target,
            project_scope: remote_scope.clone(),
            app_scope_id: remote_app.clone(),
            stable_name: "Bob".into(),
        });
        let request = FederationPut {
            origin: OriginMessageRef {
                machine: "air".into(),
                project_scope: remote_scope,
                app_scope_id: remote_app,
                operation_id: crate::identity::OperationId::new("reply-1").unwrap(),
            },
            sender,
            target: FederationDestination::ChannelOwner(channel.clone()),
            mtype: "reply".into(),
            subject: "reply".into(),
            body: "reply body".into(),
            delivery: "immediate".into(),
            target_session: None,
            in_reply_to: Some(accepted),
            original_origin: Some(record.origin.clone()),
            reply_policy: None,
            channel: Some(channel),
        };
        let connection = AuthenticatedMachine {
            source: "air".into(),
            target: "studio".into(),
        };
        assert!(validate_channel_reply(&connection, &request, &record).is_ok());
        let mut stranger = request.clone();
        let FederationActor::Peer(peer) = &mut stranger.sender else {
            unreachable!()
        };
        peer.address.local.name = "Mallory".into();
        assert!(validate_channel_reply(&connection, &stranger, &record).is_err());
    }
}
