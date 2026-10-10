fn handle_create(
    server: &Server,
    context: Option<&ProjectContext>,
    request: &PeerLifecycleRequest,
) -> Resp {
    let PeerLifecycleRequest::Create {
        worker_id,
        token,
        operation_id,
        query_capability,
        peer_id,
        cwd,
        model,
    } = request
    else {
        unreachable!()
    };
    let Some(context) = context else {
        return unavailable(request, "PROJECT_CONTEXT_REQUIRED", true);
    };
    if operation_id.trim().is_empty()
        || query_capability.trim().is_empty()
        || crate::identity::validate_id_for_protocol(peer_id).is_err()
        || model.as_ref().is_some_and(|model| model.trim().is_empty())
    {
        return unavailable(request, "PEER_LIFECYCLE_CREATE_INTENT_INVALID", true);
    }
    let cwd = match normalize_cwd(cwd) {
        Ok(cwd) => cwd,
        Err(error) => return unavailable(request, &error, true),
    };
    let route = match crate::scope::canonical_route_for_identity(
        &server.host_paths,
        Path::new(&cwd),
        &context.app_scope_id,
    ) {
        Ok(route) => route,
        Err(error) => {
            return unavailable(
                request,
                &format!("PEER_LIFECYCLE_CWD_OUT_OF_SCOPE: {error}"),
                true,
            )
        }
    };
    if route.root != PathBuf::from(context.project_scope.as_str()) {
        return unavailable(request, "PEER_LIFECYCLE_CWD_SCOPE_MISMATCH", true);
    }
    let digest = capability_hash(
        &json!({"action":"create", "actor":worker_id,
        "operation_id":operation_id, "peer_id":peer_id, "cwd":cwd, "model":model,
        "project_scope":context.project_scope, "app_scope":context.app_scope_id})
        .to_string(),
    );
    let cap_hash = capability_hash(query_capability);
    // A same-operation replay is the durable readback path: it may finalize an
    // existing pending challenge but never repeats the Native effects.
    let existing = {
        let state = server.state.lock().unwrap();
        if verify(&state, worker_id, token).is_err()
            || current_master_holder(server, &state)
                .ok()
                .flatten()
                .as_deref()
                != Some(worker_id)
        {
            return unavailable(request, "PEER_LIFECYCLE_MASTER_REQUIRED", true);
        }
        state.peer_lifecycle_operations.get(operation_id).cloned()
    };
    if let Some(existing) = existing {
        if existing.action != PeerLifecycleAction::Create
            || existing.actor_id != *worker_id
            || existing.project_scope != context.project_scope
            || existing.app_scope_id != context.app_scope_id
            || existing.intent_digest != digest
            || existing.query_capability_hash != cap_hash
        {
            return unavailable(request, "PEER_LIFECYCLE_INTENT_CONFLICT", true);
        }
        let finalized = match finalize_readback(server, context, &existing) {
            Ok(record) => record,
            Err(error) => return readback_commit_failure(server, &existing, &error),
        };
        let result = record_result(&finalized);
        return response(result.clone(), result_error(&result));
    }
    let (mut record, mut host_transport) = {
        let mut state = server.state.lock().unwrap();
        if verify(&state, worker_id, token).is_err()
            || current_master_holder(server, &state)
                .ok()
                .flatten()
                .as_deref()
                != Some(worker_id)
        {
            return unavailable(request, "PEER_LIFECYCLE_MASTER_REQUIRED", true);
        }
        if let Some(existing) = state.peer_lifecycle_operations.get(operation_id) {
            if existing.action != PeerLifecycleAction::Create
                || existing.actor_id != *worker_id
                || existing.project_scope != context.project_scope
                || existing.app_scope_id != context.app_scope_id
                || existing.intent_digest != digest
                || existing.query_capability_hash != cap_hash
            {
                return unavailable(request, "PEER_LIFECYCLE_INTENT_CONFLICT", true);
            }
            return response(
                record_result(existing),
                result_error(&record_result(existing)),
            );
        }
        if state.workers.contains_key(peer_id)
            || state.peer_lifecycle_operations.values().any(|record| {
                record.action == PeerLifecycleAction::Create
                    && record.project_scope == context.project_scope
                    && record.app_scope_id == context.app_scope_id
                    && record
                        .create
                        .as_ref()
                        .is_some_and(|create| create.peer_id == *peer_id)
                    && !matches!(
                        record.phase,
                        PeerLifecyclePhase::Complete | PeerLifecyclePhase::Refused
                    )
            })
        {
            return unavailable(request, "PEER_LIFECYCLE_PEER_RESERVED", true);
        }
        let Some(actor) = state.workers.get(worker_id) else {
            return unavailable(request, "PEER_LIFECYCLE_MASTER_REQUIRED", true);
        };
        let actor_target = match exact_target(&state, context, actor) {
            Ok(target) => target,
            Err(_) => return unavailable(request, "PEER_LIFECYCLE_MASTER_ROUTE_UNPROVEN", true),
        };
        let transport = selected_transport(&actor_target);
        if transport.kind != TransportKind::AppServer {
            return unavailable(request, "PEER_LIFECYCLE_CREATE_NATIVE_REQUIRED", true);
        }
        let now = now_ms();
        let mut record = state::PeerLifecycleOperationRecord {
            operation_id: operation_id.clone(),
            action: PeerLifecycleAction::Create,
            actor_id: worker_id.clone(),
            project_scope: context.project_scope.clone(),
            app_scope_id: context.app_scope_id.clone(),
            target: None,
            intent_digest: digest,
            query_capability_hash: cap_hash,
            responsibility_snapshot: None,
            phase: PeerLifecyclePhase::IntentPersisted,
            update: None,
            close: None,
            create: Some(PeerLifecycleCreate {
                peer_id: peer_id.clone(),
                cwd: cwd.clone(),
                model: model.clone(),
                thread_id: None,
                stages: vec!["intent_persisted".into()],
                readiness: PeerLifecycleStageReadback {
                    state: PeerLifecycleStage::NotAttempted,
                    challenge: None,
                },
            }),
            created_ms: now,
            updated_ms: now,
        };
        if let Err(error) = server.commit_locked(
            &mut state,
            &[Event::PeerLifecycleOperationRecorded {
                operation: record.clone(),
            }],
        ) {
            return unavailable(
                request,
                &format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}"),
                false,
            );
        }
        record.phase = PeerLifecyclePhase::HostDispatchClaimed;
        record
            .create
            .as_mut()
            .unwrap()
            .stages
            .push("host_dispatch_claimed".into());
        if let Err(error) = server.commit_locked(
            &mut state,
            &[Event::PeerLifecycleOperationRecorded {
                operation: record.clone(),
            }],
        ) {
            return response(
                record_result(state.peer_lifecycle_operations.get(operation_id).unwrap()),
                Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
            );
        }
        (record, transport)
    };
    // The durable claim above consumes the sole host send, including crashes and lost responses.
    let thread_id = match start_thread(&host_transport, Path::new(&cwd), model.as_deref()) {
        Ok(id) => id.as_str().to_owned(),
        Err(error) => {
            return create_failure(
                server,
                record,
                PeerLifecyclePhase::Unknown,
                &format!("PEER_LIFECYCLE_START_UNKNOWN: {error}"),
            )
        }
    };
    record.phase = PeerLifecyclePhase::HostDispatched;
    record.create.as_mut().unwrap().thread_id = Some(thread_id.clone());
    record
        .create
        .as_mut()
        .unwrap()
        .stages
        .push("exact_thread_id_persisted".into());
    if let Err(error) = create_commit(server, &record) {
        return error;
    }
    let scope = crate::scope::Scope { root: route.root };
    let mut identity = match crate::identity::draft_identity_with_id(&scope, peer_id) {
        Ok(identity) => identity,
        Err(error) => {
            return create_failure(
                server,
                record,
                PeerLifecyclePhase::Partial,
                &error.to_string(),
            )
        }
    };
    // Observe the exact newly returned thread; never infer its session from another id.
    host_transport.thread_id = Some(thread_id.clone());
    let history = match read_thread_history(&host_transport, &thread_id) {
        Ok(history) => history,
        Err(error) => {
            return create_failure(
                server,
                record,
                PeerLifecyclePhase::Partial,
                &format!("PEER_LIFECYCLE_CREATED_THREAD_READ_FAILED: {error}"),
            )
        }
    };
    let session_id = history
        .pointer("/thread/sessionId")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty());
    if history
        .pointer("/thread/id")
        .and_then(serde_json::Value::as_str)
        != Some(&thread_id)
        || session_id.is_none()
    {
        return create_failure(
            server,
            record,
            PeerLifecyclePhase::Partial,
            "PEER_LIFECYCLE_CREATED_SESSION_UNPROVEN",
        );
    }
    let session_id = session_id.unwrap().to_owned();
    host_transport.session_id = Some(session_id.clone());
    let candidates = TransportCandidates {
        appserver: Some(AppServerCandidate {
            endpoint: host_transport.endpoint.clone().unwrap_or_default(),
            namespace: host_transport.namespace.clone().unwrap_or_default(),
            session_id,
            thread_id: thread_id.clone(),
            cwd: cwd.clone(),
        }),
        ..Default::default()
    };
    let registered = register_created_peer(
        server,
        peer_id,
        &identity.token,
        context,
        candidates.appserver.as_ref().unwrap(),
    );
    if !registered.ok {
        return create_failure(
            server,
            record,
            PeerLifecyclePhase::Partial,
            &format!(
                "PEER_LIFECYCLE_REGISTER_FAILED: {}",
                registered.error.unwrap_or_default()
            ),
        );
    }
    let (target, transport, runtime) = {
        let state = server.state.lock().unwrap();
        let worker = state.workers.get(peer_id);
        let target = worker
            .ok_or(PeerLifecycleOutcome::Unknown)
            .and_then(|worker| exact_target(&state, context, worker));
        let transport = worker.and_then(|worker| worker.transport.clone());
        let binding = target.as_ref().ok().and_then(|target| {
            state
                .global
                .lookup_project(&target.project_scope)
                .and_then(|project| project.runtime_bindings.get(target.binding_id.as_str()))
                .cloned()
        });
        (target, transport, binding)
    };
    let target = match target {
        Ok(target) => target,
        Err(_) => {
            return create_failure(
                server,
                record,
                PeerLifecyclePhase::Partial,
                "PEER_LIFECYCLE_BINDING_ROUTE_UNPROVEN",
            )
        }
    };
    let Some(transport) = transport else {
        return create_failure(
            server,
            record,
            PeerLifecyclePhase::Partial,
            "PEER_LIFECYCLE_TRANSPORT_UNPROVEN",
        );
    };
    // Build the identity receipt from the exact committed binding, using its runtime owner.
    let Some(binding) = runtime else {
        return create_failure(
            server,
            record,
            PeerLifecyclePhase::Partial,
            "PEER_LIFECYCLE_RUNTIME_RECEIPT_MISSING",
        );
    };
    let runtime = crate::identity::RuntimeIdentity {
        agent_id: binding.agent_id.clone(),
        runtime_id: binding.runtime_id.clone(),
        appserver_id: binding.app_scope_id.clone(),
        endpoint_generation: binding.endpoint_generation,
        binding_id: binding.binding_id.clone(),
        session_id: binding.session_id.clone(),
        native_thread_id: binding.native_thread_id.clone(),
    };
    record.target = Some(target);
    record.phase = PeerLifecyclePhase::ReadbackPending;
    record.create.as_mut().unwrap().stages.extend([
        "registered".into(),
        "binding_and_current_route_verified".into(),
    ]);
    if let Err(error) = create_commit(server, &record) {
        return error;
    }
    if let Err(error) = crate::identity::persist_registration_at(
        &server.host_paths,
        &scope,
        &mut identity,
        runtime,
        transport.clone(),
    ) {
        return create_failure(
            server,
            record,
            PeerLifecyclePhase::Partial,
            &format!("PEER_LIFECYCLE_IDENTITY_RECEIPT_FAILED: {error}"),
        );
    }
    record
        .create
        .as_mut()
        .unwrap()
        .stages
        .push("identity_receipt_persisted".into());
    if let Err(error) = create_commit(server, &record) {
        return error;
    }
    // Persist the challenge intent and dispatch claim before the one readiness
    // effect. A lost response stays pending/unknown; it never authorizes a
    // resend of thread/start or the readiness turn.
    let (marker_file, marker_sha256, prompt) = match prepare_challenge(&cwd) {
        Ok(prepared) => prepared,
        Err(error) => return create_failure(server, record, PeerLifecyclePhase::Partial, &error),
    };
    let dispatched_ms = now_ms();
    record.phase = PeerLifecyclePhase::ReadbackPending;
    {
        let create = record.create.as_mut().unwrap();
        create.readiness = PeerLifecycleStageReadback {
            state: PeerLifecycleStage::Pending,
            challenge: Some(PeerLifecycleChallenge {
                marker_file,
                marker_sha256,
                turn_id: None,
                dispatched_ms,
                state: PeerLifecycleStage::Pending,
                cleanup: None,
                cleanup_error: None,
            }),
        };
        create
            .stages
            .push("readiness_challenge_intent_persisted".into());
    }
    if let Err(error) = create_commit(server, &record) {
        return error;
    }
    let receipt = match immediate_notify(
        &transport,
        None,
        &prompt,
        &format!("create-ready-{operation_id}"),
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            return create_failure(
                server,
                record,
                PeerLifecyclePhase::Partial,
                &format!("PEER_LIFECYCLE_READINESS_DISPATCH_FAILED: {error}"),
            )
        }
    };
    let Some(turn_id) = receipt
        .pointer("/turn/id")
        .and_then(serde_json::Value::as_str)
    else {
        return create_failure(
            server,
            record,
            PeerLifecyclePhase::Partial,
            "PEER_LIFECYCLE_READINESS_TURN_ID_MISSING",
        );
    };
    {
        let create = record.create.as_mut().unwrap();
        if let Some(challenge) = create.readiness.challenge.as_mut() {
            challenge.turn_id = Some(turn_id.to_owned());
        }
        create.stages.push(format!("readiness_turn:{turn_id}"));
    }
    if let Err(error) = create_commit(server, &record) {
        return error;
    }
    // Bounded initial observation only. Exhausting it is pending, never a
    // fabricated failure or a resend.
    for _ in 0..CHALLENGE_INITIAL_READS {
        match read_thread_history(&transport, &thread_id) {
            Ok(history) => {
                if let Some(verified_turn) = verify_create_challenge(&history, &record) {
                    if !current_target_matches(server, context, &record) {
                        break;
                    }
                    mark_create_challenge_verified(&mut record, &verified_turn);
                    record = match commit_verified_readback(server, record.clone()) {
                        Ok(record) => record,
                        Err(error) => return readback_commit_failure(server, &record, &error),
                    };
                    return response(record_result(&record), None);
                }
            }
            Err(error) => {
                return create_failure(
                    server,
                    record,
                    PeerLifecyclePhase::Partial,
                    &format!("PEER_LIFECYCLE_READINESS_READ_FAILED: {error}"),
                )
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if let Err(error) = create_commit(server, &record) {
        return error;
    }
    response(
        record_result(&record),
        Some("PEER_LIFECYCLE_READINESS_PENDING"),
    )
}

fn handle_query(
    server: &Server,
    context: Option<&ProjectContext>,
    request: &PeerLifecycleRequest,
) -> Resp {
    let PeerLifecycleRequest::Query {
        operation_id,
        query_capability,
    } = request
    else {
        unreachable!("handle_query requires a Query request");
    };
    let Some(context) = context else {
        return unavailable(request, "PROJECT_CONTEXT_REQUIRED", true);
    };
    let capability_hash = capability_hash(query_capability);
    let record = {
        let state = server.state.lock().unwrap();
        let Some(record) = state.peer_lifecycle_operations.get(operation_id).cloned() else {
            let mut missing = result(PeerLifecycleAction::Read, PeerLifecycleOutcome::Unknown);
            missing.operation_id = Some(operation_id.clone());
            return response(missing, Some("PEER_LIFECYCLE_OPERATION_UNKNOWN"));
        };
        if record.query_capability_hash != capability_hash
            || record.project_scope != context.project_scope
            || record.app_scope_id != context.app_scope_id
        {
            return unavailable(request, "PEER_LIFECYCLE_QUERY_DENIED", true);
        }
        record
    };
    let record = match finalize_readback(server, context, &record) {
        Ok(record) => record,
        Err(error) => return readback_commit_failure(server, &record, &error),
    };
    let result = record_result(&record);
    response(result.clone(), result_error(&result))
}

pub(super) fn handle(
    server: &Server,
    context: Option<&ProjectContext>,
    request: PeerLifecycleRequest,
) -> Resp {
    match &request {
        PeerLifecycleRequest::Create { .. } => handle_create(server, context, &request),
        PeerLifecycleRequest::Read { .. } => handle_read(server, context, &request),
        PeerLifecycleRequest::Update { .. } => handle_update(server, context, &request),
        PeerLifecycleRequest::Close { .. } => handle_close(server, context, &request),
        PeerLifecycleRequest::Query { .. } => handle_query(server, context, &request),
    }
}
