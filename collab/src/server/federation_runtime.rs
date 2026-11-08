use super::federation::{
    self, same_endpoint, AddressSendRequest, FederatedOutboundRecord, FederationSendOutcome,
};
use super::*;
use crate::address::{AddressContext, ResolvedAddress};
use crate::federation::auth::AuthenticatedMachine;
use crate::federation::transport::{call_until, FederationTransportStage};
use crate::federation::{FederationClient, FederationConfig};
use crate::proto::{
    AddressInput, DirectoryTarget, FederationActor, FederationDestination, FederationEndpoint,
    FederationPrincipal, FederationPut, FederationReq, FederationResponse, LocalNameAddress,
    MessageRef, MessageReference, OriginMessageRef, QualifiedNameAddress,
};

const REQUEST_DEADLINE: Duration = Duration::from_secs(15);

pub(super) fn accepted_reply_clear_event(
    state: &State,
    record: &FederatedOutboundRecord,
    outcome: &FederationSendOutcome,
    local_machine: &str,
) -> Option<Event> {
    if !matches!(outcome, FederationSendOutcome::Accepted { .. }) {
        return None;
    }
    let reference = record.in_reply_to.as_ref()?;
    if !reference.machine.eq_ignore_ascii_case(local_machine) {
        return None;
    }
    let original = state.federation.message_for_reference(reference)?;
    if original.recipient != record.sender
        || !state
            .reply_obligations
            .contains_key(reference.msg_id.as_str())
    {
        return None;
    }
    Some(Event::ReplyObligationCleared {
        message_id: reference.msg_id.to_string(),
    })
}

pub(super) async fn channel_dispatch(
    manager: Arc<ProjectRuntimeManager>,
    context: Option<ProjectContext>,
    request: Req,
) -> (Arc<Server>, Resp) {
    let host = manager.host.clone();
    channel_dispatch_inner(manager, context, request)
        .await
        .unwrap_or_else(|error| (host, Resp::err(error)))
}

async fn channel_dispatch_inner(
    manager: Arc<ProjectRuntimeManager>,
    context: Option<ProjectContext>,
    request: Req,
) -> Result<(Arc<Server>, Resp), String> {
    let context = context.ok_or("PROJECT_CONTEXT_REQUIRED")?;
    let prepare_manager = manager.clone();
    let prepare_context = context.clone();
    let prepare_request = request.clone();
    let (server, duplicate, intent) = blocking(move || {
        prepare_manager.recover_pending_effects_for_scope(&prepare_context)?;
        let server = prepare_manager.select_runtime(&prepare_context)?;
        validate_request_context(&server, &prepare_request, Some(&prepare_context))?;
        let Req::ChannelInbound {
            to, body, envelope, ..
        } = &prepare_request
        else {
            return Err("FEDERATION_REQUEST_INVALID".into());
        };
        validate_channel_envelope_for_inbound(envelope)?;
        let state = server.state.lock().unwrap();
        if state.admission_frozen() {
            return Err("PROJECT_RESET_PENDING".into());
        }
        let key = channel_envelope::platform_event_key(
            &envelope.channel_instance,
            &envelope.external_conversation,
            &envelope.platform_message_id,
        );
        if let Some(target) = state.platform_event_index.get(&key).cloned() {
            drop(state);
            return Ok((server, Some(target), None));
        }
        let configuration = prepare_manager.host.config.federation.as_ref();
        let peers: Vec<String> = configuration
            .map(|config| config.peers.keys().cloned().collect())
            .unwrap_or_default();
        let projects = project_aliases(&prepare_manager);
        let address_context = AddressContext {
            local_machine: configuration.map(|config| config.machine.as_str()),
            peers: &peers,
            local_projects: &projects,
        };
        let intent = channel_inbound_address_intent_with_context(
            &state,
            &envelope.source_channel,
            &envelope.channel_instance,
            &envelope.external_conversation,
            body,
            to,
            &address_context,
        )?;
        drop(state);
        Ok((server, None, Some(intent)))
    })
    .await?;
    if let Some(duplicate) = duplicate {
        match duplicate {
            channel_envelope::PlatformEventTarget::LocalMessage(id) => {
                return Ok((
                    server,
                    Resp::data(json!({"msg_id": id, "deduplicated": true})),
                ));
            }
            channel_envelope::PlatformEventTarget::FederatedOrigin(origin) => {
                let record = server
                    .state
                    .lock()
                    .unwrap()
                    .federation
                    .outbound_record(&origin)
                    .cloned()
                    .ok_or("FEDERATION_CHANNEL_ORIGIN_MISSING")?;
                let configuration = manager
                    .host
                    .config
                    .federation
                    .clone()
                    .ok_or("FEDERATION_NOT_CONFIGURED")?;
                let response = submit_record(manager, server.clone(), configuration, record).await;
                return Ok((server, response));
            }
        }
    }
    let intent = intent.ok_or("CHANNEL_ROUTE_UNAVAILABLE")?;
    if let ResolvedAddress::Remote { machine, target } = intent.target {
        let Req::ChannelInbound {
            from,
            subject,
            mut envelope,
            ..
        } = request
        else {
            return Err("FEDERATION_REQUEST_INVALID".into());
        };
        envelope.recipient_address = intent.address;
        let request = Req::ChannelInbound {
            from,
            to: envelope.recipient_address.clone(),
            subject,
            body: intent.body,
            envelope,
        };
        return Ok(super::federation_channel::inbound(
            manager,
            Some(context),
            request,
            QualifiedNameAddress {
                machine,
                local: target,
            },
        )
        .await);
    }
    blocking(move || Ok(manager.dispatch_sync(Some(context), request))).await
}

async fn blocking<T: Send + 'static>(
    action: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(action)
        .await
        .map_err(|error| format!("FEDERATION_HANDLER_FAILED: {error}"))?
}

fn response_error(response: Resp) -> String {
    response
        .error
        .unwrap_or_else(|| "FEDERATION_ADMISSION_FAILED".into())
}

fn wire_error(detail: impl Into<String>) -> FederationResponse {
    FederationResponse::Error {
        code: "FEDERATION_REQUEST_FAILED".into(),
        detail: detail.into(),
    }
}

pub(super) async fn dispatch(
    manager: Arc<ProjectRuntimeManager>,
    context: Option<ProjectContext>,
    request: Req,
) -> (Arc<Server>, Resp) {
    let host = manager.host.clone();
    let result = match request {
        request @ Req::AddressSend { .. } => send(manager, context, request).await,
        Req::FederationDirectory { machine, project } => {
            directory(manager.clone(), machine, project)
                .await
                .map(|response| (manager.host.clone(), response))
        }
        Req::MessageRefStatus {
            worker_id,
            token,
            command,
            reference,
        } => status(manager, context, worker_id, token, command, reference).await,
        _ => Err("FEDERATION_REQUEST_INVALID".into()),
    };
    result.unwrap_or_else(|error| (host, Resp::err(error)))
}

struct LocalActor {
    server: Arc<Server>,
    principal: FederationPrincipal,
}

fn authenticate_actor(
    manager: &ProjectRuntimeManager,
    context: &ProjectContext,
    worker: &str,
    token: &str,
    command: Option<&CommandEnvelope>,
) -> Result<LocalActor, String> {
    let server = manager.select_runtime(context)?;
    let command = command.ok_or("SEND_BINDING_REJECTED: a current command envelope is required")?;
    let state = server.state.lock().unwrap();
    if state.admission_frozen() {
        return Err("PROJECT_RESET_PENDING".into());
    }
    let (_, runtime, scope) =
        authenticated_send_admission(&state, worker, token, command, Some(&context.app_scope_id))
            .map_err(response_error)?;
    if context
        .runtime_context
        .as_ref()
        .is_some_and(|provided| provided != &runtime)
    {
        return Err(
            "SEND_BINDING_REJECTED: runtime context does not match the authenticated binding"
                .into(),
        );
    }
    if scope.project_scope_id != context.project_scope {
        return Err(
            "SEND_BINDING_REJECTED: caller project does not match the request context".into(),
        );
    }
    let control = manager.binding_control_store().load()?;
    let slot = control
        .current_for_worker(&scope.project_scope_id, &scope.app_scope_id, worker)?
        .ok_or("IDENTITY_MAP_NOT_CURRENT: sender has no current stable name")?;
    let name = slot.identity_key.name.clone();
    let project = canonical_project_key(&server.root).ok_or("ADDRESS_PROJECT_UNAVAILABLE")?;
    let machine = manager
        .host
        .config
        .federation
        .as_ref()
        .map(|config| config.machine.as_str())
        .unwrap_or("local");
    let principal = FederationPrincipal {
        address: QualifiedNameAddress {
            machine: machine.into(),
            local: LocalNameAddress {
                project,
                name: name.clone(),
            },
        },
        project_scope: scope.project_scope_id,
        app_scope_id: scope.app_scope_id,
        stable_name: name,
    };
    drop(state);
    Ok(LocalActor { server, principal })
}

fn local_directory(manager: &ProjectRuntimeManager) -> Result<Vec<DirectoryTarget>, String> {
    let response = manager.directory_all();
    if !response.ok {
        return Err(response_error(response));
    }
    serde_json::from_value(response.data["targets"].clone())
        .map_err(|error| format!("DIRECTORY_INVALID: {error}"))
}

pub(super) fn project_aliases(manager: &ProjectRuntimeManager) -> Vec<String> {
    let mut projects: Vec<_> = manager
        .routes
        .lock()
        .unwrap()
        .values()
        .filter_map(|route| canonical_project_key(&route.root))
        .collect();
    projects.sort();
    projects.dedup();
    projects
}

pub(super) fn resolve(
    manager: &ProjectRuntimeManager,
    input: &AddressInput,
) -> Result<ResolvedAddress, String> {
    let configuration = manager.host.config.federation.as_ref();
    let peers: Vec<_> = configuration
        .map(|config| config.peers.keys().cloned().collect())
        .unwrap_or_default();
    let projects = project_aliases(manager);
    crate::address::resolve_address(
        input,
        &AddressContext {
            local_machine: configuration.map(|config| config.machine.as_str()),
            peers: &peers,
            local_projects: &projects,
        },
    )
    .map_err(|error| error.to_string())
}

fn named_target(
    manager: &ProjectRuntimeManager,
    address: &LocalNameAddress,
) -> Result<DirectoryTarget, String> {
    let mut matches: Vec<_> = local_directory(manager)?
        .into_iter()
        .filter(|target| {
            target
                .project
                .as_deref()
                .is_some_and(|project| project.eq_ignore_ascii_case(&address.project))
                && target.display_name.eq_ignore_ascii_case(&address.name)
        })
        .collect();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err("ADDRESS_NOT_FOUND: no current local project/name holder".into()),
        _ => Err("ADDRESS_AMBIGUOUS: project/name resolves to multiple current holders".into()),
    }
}

fn target_context(target: &DirectoryTarget) -> Result<ProjectContext, String> {
    let root = target
        .project_root
        .as_deref()
        .ok_or("PROJECT_ROUTE_UNAVAILABLE")?;
    let app = AppServerId::new(
        target
            .app_scope
            .as_deref()
            .ok_or("PROJECT_ROUTE_UNAVAILABLE")?,
    )
    .map_err(|error| error.to_string())?;
    ProjectContext::for_registered_root_with_app(Path::new(root), app)
        .map_err(|error| error.to_string())
}

fn local_send(
    manager: &ProjectRuntimeManager,
    context: ProjectContext,
    request: Req,
    resolved: ResolvedAddress,
) -> Result<(Arc<Server>, Resp), String> {
    let Req::AddressSend {
        worker_id,
        token,
        command,
        to: _,
        mtype,
        subject,
        body,
        in_reply_to,
        delivery,
        reply_policy,
        target_session,
        operation_id,
    } = request
    else {
        return Err("FEDERATION_REQUEST_INVALID".into());
    };
    if operation_id.is_some() {
        return Err("LOCAL_OPERATION_ID_UNSUPPORTED: explicit recovery operation IDs are supported by remote sends".into());
    }
    let named = match &resolved {
        ResolvedAddress::LocalName(address) => Some(named_target(manager, &address)?),
        ResolvedAddress::LocalLegacy(ref raw) if raw.contains('.') => {
            let (project, name) = raw.rsplit_once('.').ok_or("ADDRESS_INVALID")?;
            Some(named_target(
                manager,
                &LocalNameAddress {
                    project: project.into(),
                    name: name.into(),
                },
            )?)
        }
        _ => None,
    };
    let (to, destination_context, named_worker) = match named {
        Some(target) => {
            let address = LocalNameAddress {
                project: target
                    .project
                    .clone()
                    .ok_or("ADDRESS_PROJECT_UNAVAILABLE")?,
                name: target.display_name.clone(),
            };
            (
                crate::address::format_local_address(&address)
                    .map_err(|error| error.to_string())?,
                Some(target_context(&target)?),
                Some(target.worker_id),
            )
        }
        None => match resolved {
            ResolvedAddress::LocalLegacy(raw) => (raw, None, None),
            _ => return Err("ADDRESS_INVALID".into()),
        },
    };
    let in_reply_to = match in_reply_to {
        Some(reference) => {
            let local = manager
                .host
                .config
                .federation
                .as_ref()
                .map(|config| config.machine.as_str())
                .unwrap_or("local");
            if !reference.machine.eq_ignore_ascii_case(local) {
                return Err("FEDERATION_REPLY_TARGET_MISMATCH".into());
            }
            Some(reference.msg_id.to_string())
        }
        None => None,
    };
    if let Some(destination) = destination_context.filter(|destination| {
        destination.project_scope != context.project_scope
            || destination.app_scope_id != context.app_scope_id
    }) {
        if mtype != "message" || delivery != "immediate" {
            return Err("CROSS_PROJECT_MODE_UNSUPPORTED: the current local cross-project owner accepts immediate messages".into());
        }
        return Ok(manager.dispatch_sync(
            Some(destination),
            Req::CrossProjectSend {
                from: worker_id.clone(),
                from_project: context.canonical_root,
                worker_id,
                token,
                command,
                to: named_worker.ok_or("ADDRESS_NOT_FOUND")?,
                subject,
                body,
                in_reply_to,
                reply_policy: reply_policy.map(|policy| {
                    serde_json::to_value(policy).expect("typed reply policy serializes")
                }),
                target_session: target_session.map(serde_json::Value::String),
            },
        ));
    }
    Ok(manager.dispatch_sync(
        Some(context),
        Req::Send {
            from: worker_id.clone(),
            worker_id: Some(worker_id),
            token: Some(token),
            command,
            to,
            mtype,
            subject: Some(subject),
            body,
            in_reply_to,
            delivery,
            reply_policy,
            target_session,
        },
    ))
}

async fn send(
    manager: Arc<ProjectRuntimeManager>,
    context: Option<ProjectContext>,
    request: Req,
) -> Result<(Arc<Server>, Resp), String> {
    let context = context.ok_or("PROJECT_CONTEXT_REQUIRED")?;
    let Req::AddressSend {
        worker_id,
        token,
        command,
        to,
        mtype,
        subject,
        body,
        delivery,
        operation_id,
        in_reply_to,
        reply_policy,
        target_session,
    } = request.clone()
    else {
        return Err("FEDERATION_REQUEST_INVALID".into());
    };
    let auth_manager = manager.clone();
    let auth_context = context.clone();
    let actor = blocking(move || {
        authenticate_actor(
            &auth_manager,
            &auth_context,
            &worker_id,
            &token,
            command.as_ref(),
        )
    })
    .await?;
    if !MSG_TYPES.contains(&mtype.as_str()) || mtype == "notification" {
        return Err("MESSAGE_TYPE_INVALID".into());
    }
    if !matches!(delivery.as_str(), "immediate" | "queued") {
        return Err("DELIVERY_MODE_INVALID".into());
    }
    let mut original_origin = None;
    let mut channel = None;
    let mut expected_reply = None;
    if let Some(reference) = in_reply_to.as_ref() {
        let state = actor.server.state.lock().unwrap();
        if let Some(meta) = state.federation.message_for_reference(reference) {
            if meta.recipient != FederationActor::Peer(actor.principal.clone()) {
                return Err("FEDERATION_REPLY_PARTICIPANT_MISMATCH".into());
            }
            expected_reply = Some(match &meta.sender {
                FederationActor::Peer(sender) => FederationEndpoint::Named(sender.address.clone()),
                FederationActor::ExternalChannel(channel) => {
                    FederationEndpoint::ChannelOwner(channel.clone())
                }
            });
            original_origin = Some(meta.origin.clone());
            channel = meta.channel.clone();
        } else if let Some(record) = state
            .federation
            .outbound
            .values()
            .find(|record| record.accepted.as_ref() == Some(reference))
        {
            if record.sender != FederationActor::Peer(actor.principal.clone()) {
                return Err("FEDERATION_REPLY_PARTICIPANT_MISMATCH".into());
            }
            expected_reply = Some(record.target.clone());
            original_origin = Some(record.origin.clone());
            channel = record.channel.clone();
        } else {
            return Err("FEDERATION_REPLY_REFERENCE_UNKNOWN".into());
        }
    }
    let channel_destination = match expected_reply.as_ref() {
        Some(FederationEndpoint::ChannelOwner(channel)) => {
            if to != AddressInput::Legacy(channel.reply_address()) {
                return Err("FEDERATION_REPLY_TARGET_MISMATCH: use the original external sender's reply address".into());
            }
            Some(FederationEndpoint::ChannelOwner(channel.clone()))
        }
        _ => None,
    };
    let destination = if let Some(destination) = channel_destination {
        destination
    } else {
        let route_manager = manager.clone();
        let input = to.clone();
        let resolved = blocking(move || resolve(&route_manager, &input)).await?;
        match resolved {
            ResolvedAddress::Remote { machine, target } => {
                FederationEndpoint::Named(QualifiedNameAddress {
                    machine,
                    local: target,
                })
            }
            local => {
                if expected_reply.as_ref().is_some_and(|expected| {
                    !expected
                        .machine()
                        .eq_ignore_ascii_case(&actor.principal.address.machine)
                }) {
                    return Err("FEDERATION_REPLY_TARGET_MISMATCH: an explicit remote reply destination is required".into());
                }
                let local_manager = manager.clone();
                return blocking(move || local_send(&local_manager, context, request, local)).await;
            }
        }
    };
    if expected_reply
        .as_ref()
        .is_some_and(|expected| !same_endpoint(expected, &destination))
    {
        return Err("FEDERATION_REPLY_TARGET_MISMATCH".into());
    }
    let configuration = manager
        .host
        .config
        .federation
        .clone()
        .ok_or("FEDERATION_NOT_CONFIGURED")?;
    let candidate = federation::prepare_outbound(
        AddressSendRequest {
            sender: FederationActor::Peer(actor.principal),
            target: destination,
            mtype,
            subject,
            body,
            delivery,
            operation_id,
            in_reply_to,
            original_origin,
            reply_policy,
            target_session,
            channel,
            now_ms: now_ms(),
        },
        &configuration.machine,
    )
    .map_err(|error| error.to_string())?;
    let server = actor.server;
    let response = submit_record(manager, server.clone(), configuration, candidate).await;
    Ok((server, response))
}

pub(super) async fn submit_record(
    _manager: Arc<ProjectRuntimeManager>,
    server: Arc<Server>,
    configuration: FederationConfig,
    candidate: FederatedOutboundRecord,
) -> Resp {
    let origin = candidate.origin.clone();
    match submit_record_inner(server, configuration, candidate).await {
        Ok(response) => response,
        Err(error) => Resp {
            ok: false,
            error: Some(error),
            data: json!({"outcome": "error", "origin": origin,
                "origin_ref": crate::proto::format_collab_ref(&MessageReference::Origin(origin.clone())).expect("typed reference serializes")}),
        },
    }
}

async fn submit_record_inner(
    server: Arc<Server>,
    configuration: FederationConfig,
    candidate: FederatedOutboundRecord,
) -> Result<Resp, String> {
    let prepared_server = server.clone();
    let prepared_candidate = candidate.clone();
    let (record, existing) = blocking(move || {
        let mut state = prepared_server.state.lock().unwrap();
        if state.admission_frozen() {
            return Err("PROJECT_RESET_PENDING".into());
        }
        if let Some(record) = state.federation.outbound_record(&prepared_candidate.origin) {
            let same = if record.body.is_some() {
                record.matches_request(&prepared_candidate)
            } else {
                record.matches_request_fields(&prepared_candidate)
            };
            if !same {
                return Err("FEDERATION_OPERATION_CONFLICT".into());
            }
            return Ok((record.clone(), true));
        }
        prepared_server
            .commit_locked_checked(
                &mut state,
                &[Event::FederationOutboundPrepared {
                    record: prepared_candidate.clone(),
                }],
            )
            .map_err(|error| error.to_string())?;
        Ok((prepared_candidate, false))
    })
    .await?;
    let accepted_before = record.accepted.clone();
    let request = if existing {
        FederationReq::Lookup {
            origin: record.origin.clone(),
            requester: record.sender.clone(),
            compare: if record.body.is_none() {
                candidate.to_put()
            } else {
                None
            },
        }
    } else {
        FederationReq::Put(record.to_put().ok_or("FEDERATION_BODY_UNAVAILABLE")?)
    };
    let deadline = tokio::time::Instant::now() + REQUEST_DEADLINE;
    let mut response =
        call_until(&configuration, record.target.machine(), &request, deadline).await;
    let mut cleanup_errors = Vec::new();
    if let Ok(receipt) = &response {
        if let Some(error) = &receipt.cleanup_error {
            let phase = if existing { "lookup" } else { "put" };
            cleanup_errors.push(format!("{phase} cleanup: {error}"));
        }
    }
    if response
        .as_ref()
        .is_ok_and(|receipt| matches!(&receipt.response, FederationResponse::NotSeen))
    {
        if record.status == FederatedOutboundRecord::STATUS_ACCEPTED {
            return Err("FEDERATION_ACCEPTANCE_INCONSISTENT: the accepted target no longer recognizes this operation".into());
        }
        response = call_until(
            &configuration,
            record.target.machine(),
            &FederationReq::Put(record.to_put().ok_or("FEDERATION_BODY_UNAVAILABLE")?),
            deadline,
        )
        .await;
        if let Ok(receipt) = &response {
            if let Some(error) = &receipt.cleanup_error {
                cleanup_errors.push(format!("put cleanup: {error}"));
            }
        }
    }
    let outcome = match &response {
        Ok(receipt) => federation::outcome_from_response(&record, &receipt.response),
        Err(error) if !existing && error.stage == FederationTransportStage::BeforeRequest => {
            FederationSendOutcome::Rejected {
                origin: record.origin.clone(),
                code: "MACHINE_UNREACHABLE".into(),
                detail: error.detail.clone(),
            }
        }
        Err(error) => FederationSendOutcome::Unknown {
            origin: record.origin.clone(),
            detail: error.detail.clone(),
        },
    };
    let resulting_server = server.clone();
    let update = federation::record_with_outcome(&record, &outcome, now_ms());
    let local_machine = configuration.machine.clone();
    let confirmed_outcome = outcome.clone();
    blocking(move || {
        let mut state = resulting_server.state.lock().unwrap();
        let current = state
            .federation
            .outbound_record(&update.origin)
            .ok_or("FEDERATION_OUTBOUND_MISSING")?;
        let clear_event =
            accepted_reply_clear_event(&state, &update, &confirmed_outcome, &local_machine);
        let mut events = Vec::new();
        if current.status != FederatedOutboundRecord::STATUS_ACCEPTED {
            events.push(Event::FederationOutboundResulted { record: update });
        }
        if let Some(event) = clear_event {
            events.push(event);
        }
        if !events.is_empty() {
            resulting_server
                .commit_locked_checked(&mut state, &events)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })
    .await?;
    match response {
        Ok(receipt) => {
            let cleanup_error = (!cleanup_errors.is_empty()).then(|| cleanup_errors.join("; "));
            Ok(with_transport_error(
                outbound_response(&outcome, receipt.response),
                cleanup_error,
            ))
        }
        Err(error) => {
            cleanup_errors.push(error.to_string());
            Ok(Resp::data(json!({
                "outcome": if !existing && error.stage == FederationTransportStage::BeforeRequest { "known_not_delivered" } else { "unknown" },
                "origin": record.origin,
                "origin_ref": crate::proto::format_collab_ref(&MessageReference::Origin(record.origin.clone())).map_err(|error| error.to_string())?,
                "reference": accepted_before,
                "accepted": if record.accepted.is_some() { Some(true) } else { None },
                "consumed": serde_json::Value::Null,
                "origin_durable": true,
                "transport_error": cleanup_errors.join("; "),
            })))
        }
    }
}

fn outbound_response(outcome: &FederationSendOutcome, response: FederationResponse) -> Resp {
    match outcome {
        FederationSendOutcome::Rejected { code, detail, .. } => {
            Resp::err(format!("{code}: {detail}"))
        }
        _ => public_response(response),
    }
}

pub(super) fn public_response(response: FederationResponse) -> Resp {
    match response {
        FederationResponse::Error { code, detail } => Resp::err(format!("{code}: {detail}")),
        FederationResponse::Message { receipt } => {
            let origin_ref =
                crate::proto::format_collab_ref(&MessageReference::Origin(receipt.origin.clone()))
                    .expect("typed reference serializes");
            let message_ref = crate::proto::format_collab_ref(&MessageReference::Message(
                receipt.reference.clone(),
            ))
            .expect("typed reference serializes");
            Resp::data(
                json!({ "outcome": "accepted", "accepted": true, "durable": true,
                "consumed": receipt.consumed, "consumed_by_recv": receipt.consumed,
                "state": if receipt.consumed { "read" } else { "pending" },
                "origin": receipt.origin, "reference": receipt.reference,
                "origin_ref": origin_ref, "message_ref": message_ref,
                "msg_id": receipt.reference.msg_id, "receipt": receipt }),
            )
        }
        FederationResponse::NotSeen => Resp::data(json!({"outcome": "not_seen"})),
        FederationResponse::Expired { reference } => {
            Resp::data(json!({"outcome": "expired", "reference": reference}))
        }
        response => Resp::data(json!({"federation": response})),
    }
}

pub(super) fn with_transport_error(mut response: Resp, error: Option<String>) -> Resp {
    if let Some(error) = error {
        response.data["transport_error"] = json!(error);
    }
    response
}

fn decorated_directory(
    manager: &ProjectRuntimeManager,
    project: Option<&str>,
) -> Result<Vec<DirectoryTarget>, String> {
    let mut targets = local_directory(manager)?;
    if let Some(project) = project {
        targets.retain(|target| {
            target
                .project
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(project))
        });
    }
    let Some(configuration) = manager.host.config.federation.as_ref() else {
        return Ok(targets);
    };
    let peers: Vec<_> = configuration.peers.keys().cloned().collect();
    let projects = project_aliases(manager);
    let context = AddressContext {
        local_machine: Some(&configuration.machine),
        peers: &peers,
        local_projects: &projects,
    };
    for target in &mut targets {
        let Some(project) = target.project.as_ref() else {
            continue;
        };
        let route = QualifiedNameAddress {
            machine: configuration.machine.clone(),
            local: LocalNameAddress {
                project: project.clone(),
                name: target.display_name.clone(),
            },
        };
        target.qualified_address = Some(
            crate::address::format_qualified_address(&route, &context)
                .map_err(|error| error.to_string())?,
        );
        target.machine = Some(configuration.machine.clone());
        target.route = Some(route);
    }
    Ok(targets)
}

pub(super) async fn directory(
    manager: Arc<ProjectRuntimeManager>,
    machine: Option<String>,
    project: Option<String>,
) -> Result<Resp, String> {
    let configuration = manager.host.config.federation.clone();
    let is_local = machine.as_deref().is_none_or(|name| {
        configuration
            .as_ref()
            .map(|config| config.machine.as_str())
            .unwrap_or("local")
            .eq_ignore_ascii_case(name)
    });
    if is_local {
        let targets = blocking(move || decorated_directory(&manager, project.as_deref())).await?;
        return Ok(Resp::data(
            json!({"targets": targets, "count": targets.len(), "scope": "machine"}),
        ));
    }
    let configuration = configuration.ok_or("FEDERATION_NOT_CONFIGURED")?;
    let machine = machine.expect("remote directory has a machine");
    let response = FederationClient::call(
        &configuration,
        &machine,
        &FederationReq::Directory { project },
        REQUEST_DEADLINE,
    )
    .await
    .map_err(|error| error.to_string())?;
    let cleanup_error = response.cleanup_error;
    match response.response {
        FederationResponse::Directory { targets } => {
            if targets.iter().any(|target| {
                target
                    .machine
                    .as_deref()
                    .is_none_or(|declared| !declared.eq_ignore_ascii_case(&machine))
                    || target
                        .route
                        .as_ref()
                        .is_none_or(|route| !route.machine.eq_ignore_ascii_case(&machine))
            }) {
                return Err("FEDERATION_DIRECTORY_MISMATCH: the directory contains an undeclared machine route".into());
            }
            Ok(with_transport_error(
                Resp::data(
                    json!({"targets": targets, "count": targets.len(), "machine": machine, "scope": "machine"}),
                ),
                cleanup_error,
            ))
        }
        FederationResponse::Error { code, detail } => Err(format!("{code}: {detail}")),
        _ => Err("FEDERATION_PROTOCOL_INVALID: expected a directory response".into()),
    }
}

pub(super) fn runtime_for_reference(
    manager: &ProjectRuntimeManager,
    project: &ProjectScopeId,
    app: &AppServerId,
) -> Result<Arc<Server>, String> {
    let key = (app.as_str().to_owned(), project.as_str().to_owned());
    let (root, storage) = manager
        .routes
        .lock()
        .unwrap()
        .get(&key)
        .map(|route| (route.root.clone(), route.storage_root.clone()))
        .ok_or("FEDERATION_REFERENCE_SCOPE_UNKNOWN")?;
    manager.ensure_runtime(&key, &root, &storage)
}

async fn status(
    manager: Arc<ProjectRuntimeManager>,
    context: Option<ProjectContext>,
    worker: String,
    token: String,
    command: Option<CommandEnvelope>,
    reference: MessageReference,
) -> Result<(Arc<Server>, Resp), String> {
    let context = context.ok_or("PROJECT_CONTEXT_REQUIRED")?;
    let auth_manager = manager.clone();
    let actor = blocking(move || {
        authenticate_actor(&auth_manager, &context, &worker, &token, command.as_ref())
    })
    .await?;
    let configuration = manager
        .host
        .config
        .federation
        .clone()
        .ok_or("FEDERATION_NOT_CONFIGURED")?;
    let requester = FederationActor::Peer(actor.principal);
    let request = match reference {
        MessageReference::Message(reference) => {
            if reference
                .machine
                .eq_ignore_ascii_case(&configuration.machine)
            {
                let owner_manager = manager.clone();
                let response = blocking(move || {
                    let owner = runtime_for_reference(
                        &owner_manager,
                        &reference.project_scope,
                        &reference.app_scope_id,
                    )?;
                    let state = owner.state.lock().unwrap();
                    Ok(super::federation_receipt_response(
                        &state, &reference, &requester,
                    ))
                })
                .await?;
                return Ok((actor.server, public_response(response)));
            }
            (
                reference.machine.clone(),
                FederationReq::Receipt {
                    reference,
                    requester,
                },
            )
        }
        MessageReference::Origin(origin) => {
            let state = actor.server.state.lock().unwrap();
            if let Some(meta) = state.federation.message_for_origin(&origin) {
                return Ok((
                    actor.server.clone(),
                    public_response(super::federation_receipt_response(
                        &state,
                        &meta.reference,
                        &requester,
                    )),
                ));
            }
            let record = state
                .federation
                .outbound_record(&origin)
                .ok_or("FEDERATION_ORIGIN_UNKNOWN")?;
            if record.sender != requester {
                return Err("FEDERATION_REFERENCE_PARTICIPANT_MISMATCH".into());
            }
            (
                record.target.machine().to_owned(),
                FederationReq::Lookup {
                    origin,
                    requester,
                    compare: None,
                },
            )
        }
    };
    let response = FederationClient::call(&configuration, &request.0, &request.1, REQUEST_DEADLINE)
        .await
        .map_err(|error| error.to_string())?;
    if let FederationResponse::Message { receipt } = &response.response {
        let result_server = actor.server.clone();
        let receipt = receipt.clone();
        let local_machine = configuration.machine.clone();
        blocking(move || {
            let mut state = result_server.state.lock().unwrap();
            if let Some(record) = state.federation.outbound_record(&receipt.origin).cloned() {
                let outcome = federation::outcome_from_response(
                    &record,
                    &FederationResponse::Message { receipt },
                );
                if matches!(outcome, FederationSendOutcome::Accepted { .. }) {
                    let record = federation::record_with_outcome(&record, &outcome, now_ms());
                    let clear_event =
                        accepted_reply_clear_event(&state, &record, &outcome, &local_machine);
                    let mut events = vec![Event::FederationOutboundResulted { record }];
                    if let Some(event) = clear_event {
                        events.push(event);
                    }
                    result_server
                        .commit_locked_checked(&mut state, &events)
                        .map_err(|error| error.to_string())?;
                } else {
                    return Err("FEDERATION_RESPONSE_MISMATCH".into());
                }
            }
            Ok(())
        })
        .await?;
    }
    Ok((
        actor.server,
        with_transport_error(public_response(response.response), response.cleanup_error),
    ))
}

pub(super) fn origin_owner(
    manager: &ProjectRuntimeManager,
    origin: &OriginMessageRef,
) -> Result<Option<Arc<Server>>, String> {
    local_directory(manager)?;
    let owners: Vec<_> = manager
        .runtimes()
        .into_iter()
        .filter(|runtime| {
            let state = runtime.state.lock().unwrap();
            state.federation.message_for_origin(origin).is_some()
                || state
                    .federation
                    .expired_origins
                    .contains_key(&federation::origin_key(origin))
        })
        .collect();
    match owners.len() {
        0 => Ok(None),
        1 => Ok(owners.into_iter().next()),
        _ => Err("FEDERATION_ORIGIN_AMBIGUOUS: one origin has multiple mailbox owners".into()),
    }
}

pub(super) async fn ingress(
    manager: Arc<ProjectRuntimeManager>,
    source: AuthenticatedMachine,
    request: FederationReq,
) -> FederationResponse {
    let Some(configuration) = manager.host.config.federation.clone() else {
        return wire_error("FEDERATION_NOT_CONFIGURED");
    };
    if source.target != configuration.machine {
        return wire_error("MACHINE_IDENTITY_MISMATCH");
    }
    let result = match request {
        FederationReq::Hello => {
            return FederationResponse::Hello {
                machine: configuration.machine,
                protocol: crate::proto::FEDERATION_PROTOCOL.into(),
            }
        }
        FederationReq::Directory { project } => {
            blocking(move || {
                decorated_directory(&manager, project.as_deref())
                    .map(|targets| FederationResponse::Directory { targets })
            })
            .await
        }
        FederationReq::Lookup {
            origin,
            requester,
            compare,
        } => {
            blocking(move || {
                if requester.machine() != source.source || origin.machine != source.source {
                    return Err("FEDERATION_SOURCE_MISMATCH".into());
                }
                let Some(owner) = origin_owner(&manager, &origin)? else {
                    return Ok(FederationResponse::NotSeen);
                };
                Ok(super::lookup_federated_message(
                    &owner,
                    &configuration.machine,
                    &source,
                    &origin,
                    &requester,
                    compare.as_ref(),
                ))
            })
            .await
        }
        FederationReq::Receipt {
            reference,
            requester,
        } => {
            blocking(move || {
                let owner = runtime_for_reference(
                    &manager,
                    &reference.project_scope,
                    &reference.app_scope_id,
                )?;
                Ok(super::federated_message_receipt(
                    &owner,
                    &configuration.machine,
                    &source,
                    &reference,
                    &requester,
                ))
            })
            .await
        }
        FederationReq::Put(request) => {
            if let Err(error) = federation::verify_source(&source, &configuration.machine, &request)
            {
                return wire_error(error.to_string());
            }
            if matches!(&request.target, FederationDestination::ChannelOwner(_)) {
                return super::federation_channel::accept_reply(manager, source, request).await;
            }
            let known_manager = manager.clone();
            let known_request = request.clone();
            let known_source = source.clone();
            let known_machine = configuration.machine.clone();
            let existing = blocking(move || {
                Ok(
                    origin_owner(&known_manager, &known_request.origin)?.map(|owner| {
                        super::lookup_federated_message(
                            &owner,
                            &known_machine,
                            &known_source,
                            &known_request.origin,
                            &known_request.sender,
                            Some(&known_request),
                        )
                    }),
                )
            })
            .await;
            match existing {
                Ok(Some(response)) => return response,
                Err(error) => return wire_error(error),
                Ok(None) => {}
            }
            if let Err(error) = verify_reply(&manager, &configuration, &request).await {
                return wire_error(error);
            }
            blocking(move || {
                let _accept = manager.federation_accept_gate.lock().unwrap();
                if let Some(owner) = origin_owner(&manager, &request.origin)? {
                    return Ok(super::lookup_federated_message(
                        &owner,
                        &configuration.machine,
                        &source,
                        &request.origin,
                        &request.sender,
                        Some(&request),
                    ));
                }
                let FederationDestination::Named(address) = &request.target else {
                    return Err("FEDERATION_TARGET_INVALID".into());
                };
                let target = named_target(&manager, address)?;
                let owner = manager.select_runtime(&target_context(&target)?)?;
                let recipient = {
                    let state = owner.state.lock().unwrap();
                    super::resolve_federated_local_recipient(
                        &owner,
                        &state,
                        &configuration.machine,
                        address,
                    )
                    .map_err(|error| error.to_string())?
                };
                Ok(super::accept_federated_message(
                    &owner,
                    &configuration.machine,
                    &source,
                    &request,
                    &recipient,
                    now_ms(),
                ))
            })
            .await
        }
    };
    result.unwrap_or_else(wire_error)
}

async fn verify_reply(
    manager: &Arc<ProjectRuntimeManager>,
    configuration: &FederationConfig,
    request: &FederationPut,
) -> Result<(), String> {
    let Some(reference) = request.in_reply_to.as_ref() else {
        if request.original_origin.is_some() {
            return Err("FEDERATION_REPLY_REFERENCE_REQUIRED".into());
        }
        return Ok(());
    };
    if reference
        .machine
        .eq_ignore_ascii_case(&configuration.machine)
    {
        let owner =
            runtime_for_reference(manager, &reference.project_scope, &reference.app_scope_id)?;
        let state = owner.state.lock().unwrap();
        let meta = state
            .federation
            .message_for_reference(reference)
            .ok_or("FEDERATION_REPLY_REFERENCE_UNKNOWN")?;
        if request.original_origin.as_ref() != Some(&meta.origin)
            || !federation::reply_participant_ok(meta, &request.sender)
        {
            return Err("FEDERATION_REPLY_PARTICIPANT_MISMATCH".into());
        }
        let other = if request.sender == meta.sender {
            &meta.recipient
        } else {
            &meta.sender
        };
        let expected = match other {
            FederationActor::Peer(peer) => FederationDestination::Named(peer.address.local.clone()),
            FederationActor::ExternalChannel(channel) => {
                FederationDestination::ChannelOwner(channel.clone())
            }
        };
        if request.target != expected {
            return Err("FEDERATION_REPLY_TARGET_MISMATCH".into());
        }
        return Ok(());
    }
    let origin = request
        .original_origin
        .as_ref()
        .ok_or("FEDERATION_REPLY_ORIGIN_REQUIRED")?;
    if !origin.machine.eq_ignore_ascii_case(&configuration.machine) {
        return Err("FEDERATION_REPLY_ORIGIN_MISMATCH".into());
    }
    let owner = runtime_for_reference(manager, &origin.project_scope, &origin.app_scope_id)?;
    let mut record = {
        let state = owner.state.lock().unwrap();
        state
            .federation
            .outbound_record(origin)
            .cloned()
            .ok_or("FEDERATION_REPLY_ORIGIN_UNKNOWN")?
    };
    if record.accepted.is_none() {
        let response = FederationClient::call(
            configuration,
            record.target.machine(),
            &FederationReq::Lookup {
                origin: origin.clone(),
                requester: record.sender.clone(),
                compare: None,
            },
            REQUEST_DEADLINE,
        )
        .await
        .map_err(|error| error.to_string())?;
        if let Some(error) = &response.cleanup_error {
            eprintln!("FEDERATION_CONNECTION_CLEANUP_FAILED: reply verification: {error}");
        }
        let outcome = federation::outcome_from_response(&record, &response.response);
        if !matches!(outcome, FederationSendOutcome::Accepted { .. }) {
            return Err("FEDERATION_REPLY_ORIGIN_UNCONFIRMED".into());
        }
        record = federation::record_with_outcome(&record, &outcome, now_ms());
        let result_owner = owner.clone();
        let recovered = record.clone();
        blocking(move || {
            result_owner
                .commit_checked(&[Event::FederationOutboundResulted { record: recovered }])
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
        .await?;
    }
    let expected_sender = match &request.sender {
        FederationActor::Peer(peer) => FederationEndpoint::Named(peer.address.clone()),
        _ => return Err("FEDERATION_REPLY_PARTICIPANT_MISMATCH".into()),
    };
    let expected_target = match &record.sender {
        FederationActor::Peer(peer) => FederationDestination::Named(peer.address.local.clone()),
        FederationActor::ExternalChannel(channel) => {
            FederationDestination::ChannelOwner(channel.clone())
        }
    };
    if record.accepted.as_ref() != Some(reference)
        || !same_endpoint(&record.target, &expected_sender)
        || request.sender.project_scope() != &reference.project_scope
        || request.sender.app_scope_id() != &reference.app_scope_id
        || request.target != expected_target
        || request.channel != record.channel
    {
        return Err("FEDERATION_REPLY_PARTICIPANT_MISMATCH".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_message_reference_status_preserves_expired_result() {
        let scope = crate::scope::ProjectScopeId::new("/projects/expired").unwrap();
        let app = AppServerId::new("appserver-expired").unwrap();
        let origin = OriginMessageRef {
            machine: "studio".into(),
            project_scope: scope.clone(),
            app_scope_id: app.clone(),
            operation_id: crate::identity::OperationId::new("expired-operation").unwrap(),
        };
        let reference = MessageRef {
            machine: "air".into(),
            project_scope: scope.clone(),
            app_scope_id: app.clone(),
            msg_id: crate::identity::MessageId::new("expired-message").unwrap(),
        };
        let requester = FederationActor::Peer(FederationPrincipal {
            address: QualifiedNameAddress {
                machine: "studio".into(),
                local: LocalNameAddress {
                    project: "expired".into(),
                    name: "Alice".into(),
                },
            },
            project_scope: scope,
            app_scope_id: app,
            stable_name: "Alice".into(),
        });
        let mut state = State::default();
        state.federation.expired_origins.insert(
            federation::origin_key(&origin),
            federation::FederatedExpiredOrigin {
                origin,
                reference: reference.clone(),
                expired_ms: 1,
            },
        );
        let response = super::super::federation_receipt_response(&state, &reference, &requester);
        assert!(
            matches!(response, FederationResponse::Expired { reference: expired } if expired == reference)
        );
    }

    #[test]
    fn mismatched_target_receipt_cannot_be_reported_as_accepted() {
        let origin = OriginMessageRef {
            machine: "studio".into(),
            project_scope: crate::scope::ProjectScopeId::new("/projects/alpha").unwrap(),
            app_scope_id: AppServerId::new("appserver-alpha").unwrap(),
            operation_id: crate::identity::OperationId::new("receipt-check").unwrap(),
        };
        let sender = FederationActor::Peer(FederationPrincipal {
            address: QualifiedNameAddress {
                machine: "studio".into(),
                local: LocalNameAddress {
                    project: "alpha".into(),
                    name: "Alice".into(),
                },
            },
            project_scope: origin.project_scope.clone(),
            app_scope_id: origin.app_scope_id.clone(),
            stable_name: "Alice".into(),
        });
        let target = FederationEndpoint::Named(QualifiedNameAddress {
            machine: "air".into(),
            local: LocalNameAddress {
                project: "alpha".into(),
                name: "Bob".into(),
            },
        });
        let record = federation::prepare_outbound(
            AddressSendRequest {
                sender: sender.clone(),
                target,
                mtype: "message".into(),
                subject: "test".into(),
                body: "body".into(),
                delivery: "immediate".into(),
                operation_id: Some(origin.operation_id.clone()),
                in_reply_to: None,
                original_origin: None,
                reply_policy: None,
                target_session: None,
                channel: None,
                now_ms: 1,
            },
            &origin.machine,
        )
        .unwrap();
        let response = FederationResponse::Message {
            receipt: crate::proto::FederationMessageReceipt {
                origin: record.origin.clone(),
                reference: MessageRef {
                    machine: "air".into(),
                    project_scope: origin.project_scope,
                    app_scope_id: origin.app_scope_id,
                    msg_id: crate::identity::MessageId::new("wrong-recipient-message").unwrap(),
                },
                sender,
                recipient: FederationEndpoint::Named(QualifiedNameAddress {
                    machine: "air".into(),
                    local: LocalNameAddress {
                        project: "alpha".into(),
                        name: "Mallory".into(),
                    },
                }),
                notification: crate::proto::FederationNotificationStatus::NotAttempted,
                consumed: false,
                reply_reference: None,
                channel_delivery: None,
                channel_delivery_error: None,
            },
        };
        let outcome = federation::outcome_from_response(&record, &response);
        assert!(matches!(outcome, FederationSendOutcome::Rejected { .. }));
        let result = outbound_response(&outcome, response);
        assert!(
            !result.ok,
            "a mismatched receipt must not reach the public accepted response"
        );
        assert!(result
            .error
            .unwrap()
            .contains("FEDERATION_RESPONSE_MISMATCH"));
    }
}
