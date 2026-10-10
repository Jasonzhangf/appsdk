fn handle_read(
    server: &Server,
    context: Option<&ProjectContext>,
    request: &PeerLifecycleRequest,
) -> Resp {
    let PeerLifecycleRequest::Read {
        worker_id,
        token,
        target_id,
    } = request
    else {
        unreachable!("handle_read requires a Read request");
    };
    let Some(context) = context else {
        return unavailable(request, "PROJECT_CONTEXT_REQUIRED", true);
    };
    let target_id = target_id.as_deref().unwrap_or(worker_id);
    let mut read = result(PeerLifecycleAction::Read, PeerLifecycleOutcome::Unknown);
    let (worker, target, tasks, msgs, keepalives, closure) = {
        let state = server.state.lock().unwrap();
        if verify(&state, worker_id, token).is_err()
            || !authorized(server, &state, worker_id, target_id)
        {
            return unavailable(request, "PEER_LIFECYCLE_UNAUTHORIZED", true);
        }
        let Some(worker) = state.workers.get(target_id) else {
            if let Some(record) = state
                .peer_lifecycle_operations
                .values()
                .filter(|record| {
                    record.action == PeerLifecycleAction::Close
                        && record.exact_target().worker_id == target_id
                        && record.project_scope == context.project_scope
                        && record.app_scope_id == context.app_scope_id
                })
                .max_by_key(|record| record.created_ms)
                .cloned()
            {
                read.target = Some(record.exact_target().clone());
                read.source = PeerLifecycleSource::LifecycleOperation;
                read.close = record.close.clone();
                read.outcome = match record.phase {
                    PeerLifecyclePhase::Complete => PeerLifecycleOutcome::Closed,
                    PeerLifecyclePhase::CleanupOpen | PeerLifecyclePhase::Partial => {
                        PeerLifecycleOutcome::CleanupOpen
                    }
                    PeerLifecyclePhase::HostDispatchClaimed | PeerLifecyclePhase::Unknown => {
                        PeerLifecycleOutcome::Unknown
                    }
                    PeerLifecyclePhase::Refused | PeerLifecyclePhase::Cancelled => {
                        PeerLifecycleOutcome::Missing
                    }
                    PeerLifecyclePhase::IntentPersisted
                    | PeerLifecyclePhase::HostDispatched
                    | PeerLifecyclePhase::ReadbackPending => PeerLifecycleOutcome::CleanupOpen,
                };
                return response(read, None);
            }
            if state.worker_closures.contains_key(target_id) {
                read.source = PeerLifecycleSource::LegacyCloseReceipt;
                read.close = Some(legacy_close());
                let bindings: Vec<_> = state
                    .global
                    .projects
                    .values()
                    .flat_map(|project| project.runtime_bindings.values())
                    .filter(|binding| binding.agent_id.as_str() == target_id)
                    .collect();
                if bindings.len() == 1 {
                    let binding = bindings[0];
                    if binding.project_scope != context.project_scope
                        || binding.app_scope_id != context.app_scope_id
                    {
                        return unavailable(request, "PEER_LIFECYCLE_SCOPE_MISMATCH", true);
                    }
                    if state.responsibility_fences.values().any(|fence| {
                        fence.is_active()
                            && fence.matches_binding(
                                target_id,
                                binding.project_scope.as_str(),
                                binding.app_scope_id.as_str(),
                                binding.binding_id.as_str(),
                                binding.endpoint_generation,
                            )
                    }) {
                        read.outcome = PeerLifecycleOutcome::CleanupOpen;
                    }
                }
            } else {
                read.outcome = PeerLifecycleOutcome::Missing;
            }
            return response(read, None);
        };
        let target = match exact_target(&state, context, worker) {
            Ok(target) => target,
            Err(outcome) => {
                read.outcome = outcome;
                return response(read, Some("PEER_LIFECYCLE_TARGET_UNPROVEN"));
            }
        };
        (
            worker.clone(),
            target,
            state.tasks.clone(),
            state.msgs.clone(),
            state.keepalives.clone(),
            state.worker_closures.get(target_id).cloned(),
        )
    };
    // Reuse the public status/presence owner outside the reducer lock. The
    // narrow role projection excludes unrelated parent/master instructions.
    let mut projection = worker_status_summary_with_maps(
        server,
        &tasks,
        &msgs,
        &keepalives,
        json!({"role": "peer"}),
        &worker,
    );
    let (fenced, recorded_close) = {
        let state = server.state.lock().unwrap();
        if verify(&state, worker_id, token).is_err()
            || !authorized(server, &state, worker_id, target_id)
        {
            return unavailable(request, "PEER_LIFECYCLE_AUTHORITY_CHANGED", true);
        }
        if state.workers.get(target_id) != Some(&worker)
            || exact_target(&state, context, &worker).ok().as_ref() != Some(&target)
            || state.worker_closures.get(target_id) != closure.as_ref()
        {
            return unavailable(request, "PEER_LIFECYCLE_TARGET_CHANGED", false);
        }
        let role = if current_master_holder(server, &state)
            .ok()
            .flatten()
            .as_deref()
            == Some(target_id)
        {
            "master"
        } else {
            "peer"
        };
        projection["role"] = json!(role);
        projection["role_brief"] = json!({"role": role});
        let managed: Vec<_> = state.subagents.values().filter(|record| record.peer == target_id)
            .map(|record| json!({"id": record.id, "parent": record.parent, "peer": record.peer, "status": record.status})).collect();
        if !managed.is_empty() {
            projection["managed"] = json!(managed);
        }
        let fenced = state.responsibility_fences.values().any(|fence| {
            fence.is_active()
                && fence.matches_binding(
                    target_id,
                    target.project_scope.as_str(),
                    target.app_scope_id.as_str(),
                    target.binding_id.as_str(),
                    target.endpoint_generation,
                )
        });
        // Any recorded Close that is not a clean refusal/cancellation keeps the
        // target in its unresolved retirement state, even while the worker
        // projection still exists. Read must surface that, not a plain `ok`.
        let recorded_close = state
            .peer_lifecycle_operations
            .values()
            .filter(|record| {
                record.action == PeerLifecycleAction::Close
                    && record.exact_target().worker_id == target_id
                    && record.project_scope == context.project_scope
                    && record.app_scope_id == context.app_scope_id
                    && !matches!(
                        record.phase,
                        PeerLifecyclePhase::Refused
                            | PeerLifecyclePhase::Cancelled
                            | PeerLifecyclePhase::Complete
                    )
            })
            .max_by_key(|record| record.created_ms)
            .cloned();
        (fenced, recorded_close)
    };
    read.outcome = match projection["presence"].as_str() {
        Some("present" | "cold") => PeerLifecycleOutcome::Ok,
        Some("missing") => PeerLifecycleOutcome::Missing,
        _ => PeerLifecycleOutcome::Unknown,
    };
    read.source = PeerLifecycleSource::ExactHostObservation;
    if let Some(record) = recorded_close {
        read.source = PeerLifecycleSource::LifecycleOperation;
        read.close = record.close.clone();
        read.outcome = match record.phase {
            PeerLifecyclePhase::HostDispatchClaimed | PeerLifecyclePhase::Unknown => {
                PeerLifecycleOutcome::Unknown
            }
            _ => PeerLifecycleOutcome::CleanupOpen,
        };
    } else if closure.is_some() {
        read.source = PeerLifecycleSource::LegacyCloseReceipt;
        read.close = Some(legacy_close());
        read.outcome = if fenced {
            PeerLifecycleOutcome::CleanupOpen
        } else {
            PeerLifecycleOutcome::Unknown
        };
    }
    read.target = Some(target);
    read.projection = Some(projection);
    response(read, None)
}

fn handle_update(
    server: &Server,
    context: Option<&ProjectContext>,
    request: &PeerLifecycleRequest,
) -> Resp {
    let PeerLifecycleRequest::Update {
        worker_id,
        token,
        operation_id,
        query_capability,
        target,
        cwd,
    } = request
    else {
        unreachable!("handle_update requires an Update request");
    };
    let Some(context) = context else {
        return unavailable(request, "PROJECT_CONTEXT_REQUIRED", true);
    };
    let normalized_cwd = match normalize_cwd(cwd) {
        Ok(cwd) => cwd,
        Err(error) => {
            let mut refused = result(PeerLifecycleAction::Update, PeerLifecycleOutcome::Refused);
            refused.operation_id = Some(operation_id.clone());
            return response(refused, Some(&error));
        }
    };
    let digest = update_intent_digest(operation_id, target, &normalized_cwd);
    let capability_hash = capability_hash(query_capability);
    // A same-operation replay is the durable readback path: it may finalize an
    // existing pending challenge but never repeats the settings effect.
    let existing = {
        let state = server.state.lock().unwrap();
        if verify(&state, worker_id, token).is_err()
            || !authorized(server, &state, worker_id, &target.worker_id)
        {
            return unavailable(request, "PEER_LIFECYCLE_UNAUTHORIZED", true);
        }
        state.peer_lifecycle_operations.get(operation_id).cloned()
    };
    if let Some(existing) = existing {
        if existing.action != PeerLifecycleAction::Update
            || existing.actor_id != *worker_id
            || existing.project_scope != context.project_scope
            || existing.app_scope_id != context.app_scope_id
            || existing.intent_digest != digest
            || existing.query_capability_hash != capability_hash
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
    let (worker, snapshot) = {
        let state = server.state.lock().unwrap();
        if verify(&state, worker_id, token).is_err()
            || !authorized(server, &state, worker_id, &target.worker_id)
        {
            return unavailable(request, "PEER_LIFECYCLE_UNAUTHORIZED", true);
        }
        let Some(worker) = state.workers.get(&target.worker_id).cloned() else {
            return unavailable(request, "PEER_LIFECYCLE_TARGET_MISSING", true);
        };
        let committed = match exact_target(&state, context, &worker) {
            Ok(target) => target,
            Err(outcome) => {
                let mut failed = result(PeerLifecycleAction::Update, outcome);
                failed.operation_id = Some(operation_id.clone());
                return response(failed, Some("PEER_LIFECYCLE_TARGET_UNPROVEN"));
            }
        };
        if committed != *target {
            return unavailable(request, "PEER_LIFECYCLE_TARGET_MISMATCH", true);
        }
        if target.project_scope != context.project_scope
            || target.app_scope_id != context.app_scope_id
        {
            return unavailable(request, "PEER_LIFECYCLE_SCOPE_MISMATCH", true);
        }
        (
            worker,
            state::responsibility_snapshot(&state, &target.worker_id),
        )
    };
    if let Some(requires) = responsibility_requires(&snapshot) {
        let mut refused = result(PeerLifecycleAction::Update, PeerLifecycleOutcome::Refused);
        refused.operation_id = Some(operation_id.clone());
        refused.requires = requires;
        return response(refused, Some("PEER_LIFECYCLE_RESPONSIBILITY_CONFLICT"));
    }
    let route = match crate::scope::canonical_route_for_identity(
        &server.host_paths,
        Path::new(&normalized_cwd),
        &target.app_scope_id,
    ) {
        Ok(route) => route,
        Err(error) => {
            let mut refused = result(PeerLifecycleAction::Update, PeerLifecycleOutcome::Refused);
            refused.operation_id = Some(operation_id.clone());
            return response(
                refused,
                Some(&format!("PEER_LIFECYCLE_CWD_OUT_OF_SCOPE: {error}")),
            );
        }
    };
    if route.root != PathBuf::from(target.project_scope.as_str()) {
        let mut refused = result(PeerLifecycleAction::Update, PeerLifecycleOutcome::Refused);
        refused.operation_id = Some(operation_id.clone());
        return response(refused, Some("PEER_LIFECYCLE_CWD_SCOPE_MISMATCH"));
    }
    let Some(thread_id) = target.transport.thread_id.as_deref() else {
        let mut refused = result(PeerLifecycleAction::Update, PeerLifecycleOutcome::Refused);
        refused.operation_id = Some(operation_id.clone());
        return response(refused, Some("PEER_LIFECYCLE_THREAD_MISSING"));
    };
    let host_transport = selected_transport(target);
    // The registration cwd is the exact pre-update execution directory. It is
    // read locally so no selected-host interaction happens before the durable
    // intent commit.
    let previous_cwd = std::fs::canonicalize(&worker.cwd)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| worker.cwd.clone());
    let now = now_ms();
    let mut record = state::PeerLifecycleOperationRecord {
        operation_id: operation_id.clone(),
        action: PeerLifecycleAction::Update,
        actor_id: worker_id.clone(),
        project_scope: context.project_scope.clone(),
        app_scope_id: context.app_scope_id.clone(),
        target: Some(target.clone()),
        intent_digest: digest,
        query_capability_hash: capability_hash,
        responsibility_snapshot: None,
        phase: PeerLifecyclePhase::IntentPersisted,
        update: Some(PeerLifecycleUpdate {
            previous_cwd,
            intended_cwd: normalized_cwd.clone(),
            settings: PeerLifecycleSettings {
                state: PeerSettingsState::NotAttempted,
            },
            effective_cwd: PeerEffectiveCwd::Unproven,
            challenge: None,
        }),
        close: None,
        create: None,
        created_ms: now,
        updated_ms: now,
    };
    if let Err(error) = commit_record(server, &record) {
        let result = record_result(&record);
        return response(
            result,
            Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
        );
    }
    let dispatched = advance_record(
        &record,
        PeerLifecyclePhase::HostDispatched,
        record.update.clone(),
    );
    if let Err(error) = commit_record(server, &dispatched) {
        let result = record_result(&dispatched);
        return response(
            result,
            Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
        );
    }
    record = dispatched;
    let settings = update_thread_cwd(&host_transport, thread_id, &normalized_cwd);
    let target_unchanged = {
        let state = server.state.lock().unwrap();
        verify(&state, worker_id, token).is_ok()
            && state
                .workers
                .get(&target.worker_id)
                .and_then(|worker| exact_target(&state, context, worker).ok())
                .as_ref()
                == Some(target)
            && state
                .peer_lifecycle_operations
                .get(operation_id)
                .is_some_and(|stored| stored.phase.is_in_flight())
    };
    if !target_unchanged {
        let update = update_receipt(
            &record,
            PeerSettingsState::Unknown,
            PeerEffectiveCwd::Unknown,
            cleanup_record_challenge(&record),
        );
        let terminal = advance_record(&record, PeerLifecyclePhase::Unknown, update);
        let _ = commit_record(server, &terminal);
        return response(
            record_result(&terminal),
            Some("PEER_LIFECYCLE_TARGET_CHANGED"),
        );
    }
    match settings {
        ThreadSettingsUpdate::Refused(detail) => {
            let update = update_receipt(
                &record,
                PeerSettingsState::Refused,
                PeerEffectiveCwd::Unproven,
                None,
            );
            let terminal = advance_record(&record, PeerLifecyclePhase::Refused, update);
            if let Err(error) = commit_record(server, &terminal) {
                return response(
                    record_result(&terminal),
                    Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
                );
            }
            response(record_result(&terminal), Some(&detail))
        }
        ThreadSettingsUpdate::Unknown(detail) => {
            let update = update_receipt(
                &record,
                PeerSettingsState::Unknown,
                PeerEffectiveCwd::Unproven,
                None,
            );
            let terminal = advance_record(&record, PeerLifecyclePhase::Unknown, update);
            if let Err(error) = commit_record(server, &terminal) {
                return response(
                    record_result(&terminal),
                    Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
                );
            }
            response(record_result(&terminal), Some(&detail))
        }
        ThreadSettingsUpdate::Acknowledged(_) => {
            // The settings ACK is never proof of the effective cwd. Persist the
            // challenge intent and dispatch claim before the one challenge
            // turn, then accept only the exact correlated execution result.
            let (marker_file, marker_sha256, prompt) = match prepare_challenge(&normalized_cwd) {
                Ok(prepared) => prepared,
                Err(error) => {
                    let update = update_receipt(
                        &record,
                        PeerSettingsState::Acknowledged,
                        PeerEffectiveCwd::Unproven,
                        None,
                    );
                    let failed = advance_record(&record, PeerLifecyclePhase::Partial, update);
                    let _ = commit_record(server, &failed);
                    return response(record_result(&failed), Some(&error));
                }
            };
            let dispatched_ms = now_ms();
            let pending_update = update_receipt(
                &record,
                PeerSettingsState::Acknowledged,
                PeerEffectiveCwd::Unproven,
                Some(PeerLifecycleChallenge {
                    marker_file,
                    marker_sha256,
                    turn_id: None,
                    dispatched_ms,
                    state: PeerLifecycleStage::Pending,
                    cleanup: None,
                    cleanup_error: None,
                }),
            );
            let pending =
                advance_record(&record, PeerLifecyclePhase::ReadbackPending, pending_update);
            if let Err(error) = commit_record(server, &pending) {
                let _ = cleanup_record_challenge(&pending);
                return response(
                    record_result(&pending),
                    Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
                );
            }
            record = pending;
            let receipt = match immediate_notify(
                &host_transport,
                None,
                &prompt,
                &format!("update-challenge-{operation_id}"),
            ) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let update = update_receipt(
                        &record,
                        PeerSettingsState::Acknowledged,
                        PeerEffectiveCwd::Unproven,
                        cleanup_record_challenge(&record),
                    );
                    let failed = advance_record(&record, PeerLifecyclePhase::Partial, update);
                    let _ = commit_record(server, &failed);
                    return response(
                        record_result(&failed),
                        Some(&format!(
                            "PEER_LIFECYCLE_UPDATE_CHALLENGE_DISPATCH_FAILED: {error}"
                        )),
                    );
                }
            };
            let Some(turn_id) = receipt
                .pointer("/turn/id")
                .and_then(serde_json::Value::as_str)
            else {
                let update = update_receipt(
                    &record,
                    PeerSettingsState::Acknowledged,
                    PeerEffectiveCwd::Unproven,
                    cleanup_record_challenge(&record),
                );
                let failed = advance_record(&record, PeerLifecyclePhase::Partial, update);
                let _ = commit_record(server, &failed);
                return response(
                    record_result(&failed),
                    Some("PEER_LIFECYCLE_UPDATE_CHALLENGE_TURN_ID_MISSING"),
                );
            };
            if let Some(challenge) = record
                .update
                .as_mut()
                .and_then(|update| update.challenge.as_mut())
            {
                challenge.turn_id = Some(turn_id.to_owned());
            }
            if let Err(error) = commit_record(server, &record) {
                return response(
                    record_result(&record),
                    Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
                );
            }
            for _ in 0..CHALLENGE_INITIAL_READS {
                match read_thread_history(&host_transport, thread_id) {
                    Ok(history) => {
                        if let Some(verified_turn) = verify_update_challenge(&history, &record) {
                            if !current_target_matches(server, context, &record) {
                                break;
                            }
                            mark_update_challenge_verified(&mut record, thread_id, &verified_turn);
                            record = match commit_verified_readback(server, record.clone()) {
                                Ok(record) => record,
                                Err(error) => {
                                    let stored = server
                                        .state
                                        .lock()
                                        .unwrap()
                                        .peer_lifecycle_operations
                                        .get(operation_id)
                                        .cloned();
                                    let fallback = result(
                                        PeerLifecycleAction::Update,
                                        PeerLifecycleOutcome::Unknown,
                                    );
                                    return response(
                                        stored.as_ref().map(record_result).unwrap_or(fallback),
                                        Some(&format!(
                                            "PEER_LIFECYCLE_READBACK_COMMIT_FAILED: {error}"
                                        )),
                                    );
                                }
                            };
                            return response(record_result(&record), None);
                        }
                    }
                    Err(error) => {
                        let update = update_receipt(
                            &record,
                            PeerSettingsState::Acknowledged,
                            PeerEffectiveCwd::Unknown,
                            cleanup_record_challenge(&record),
                        );
                        let failed = advance_record(&record, PeerLifecyclePhase::Unknown, update);
                        let _ = commit_record(server, &failed);
                        return response(record_result(&failed), Some(&error.to_string()));
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if let Err(error) = commit_record(server, &record) {
                return response(
                    record_result(&record),
                    Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
                );
            }
            response(
                record_result(&record),
                Some("PEER_LIFECYCLE_EFFECTIVE_CWD_PENDING"),
            )
        }
    }
}

fn handle_close(
    server: &Server,
    context: Option<&ProjectContext>,
    request: &PeerLifecycleRequest,
) -> Resp {
    let PeerLifecycleRequest::Close {
        worker_id,
        token,
        operation_id,
        query_capability,
        target,
        reason,
    } = request
    else {
        unreachable!("handle_close requires a Close request");
    };
    let Some(context) = context else {
        return unavailable(request, "PROJECT_CONTEXT_REQUIRED", true);
    };
    if reason.trim().is_empty() {
        let mut refused = result(PeerLifecycleAction::Close, PeerLifecycleOutcome::Refused);
        refused.operation_id = Some(operation_id.clone());
        return response(refused, Some("PEER_LIFECYCLE_REASON_REQUIRED"));
    }
    let digest = close_intent_digest(operation_id, target, reason);
    let capability_hash = capability_hash(query_capability);
    let (record, snapshot_captured_ms) = {
        let state = server.state.lock().unwrap();
        if verify(&state, worker_id, token).is_err() {
            return unavailable(request, "PEER_LIFECYCLE_UNAUTHORIZED", true);
        }
        if let Some(existing) = state.peer_lifecycle_operations.get(operation_id) {
            if existing.action != PeerLifecycleAction::Close
                || existing.actor_id != *worker_id
                || existing.project_scope != context.project_scope
                || existing.app_scope_id != context.app_scope_id
                || existing.intent_digest != digest
                || existing.query_capability_hash != capability_hash
            {
                return unavailable(request, "PEER_LIFECYCLE_INTENT_CONFLICT", true);
            }
            return response(
                record_result(existing),
                result_error(&record_result(existing)),
            );
        }
        if !close_authorized(server, &state, worker_id, &target.worker_id) {
            return unavailable(request, "PEER_LIFECYCLE_CLOSE_UNAUTHORIZED", true);
        }
        let Some(worker) = state.workers.get(&target.worker_id).cloned() else {
            if let Some(receipt) = state.worker_closures.get(&target.worker_id) {
                let mut historical =
                    result(PeerLifecycleAction::Close, PeerLifecycleOutcome::Unknown);
                historical.operation_id = Some(operation_id.clone());
                historical.source = PeerLifecycleSource::LegacyCloseReceipt;
                historical.close = Some(legacy_close());
                let fenced = state.responsibility_fences.values().any(|fence| {
                    fence.is_active()
                        && fence.matches_binding(
                            &receipt.worker_id,
                            target.project_scope.as_str(),
                            target.app_scope_id.as_str(),
                            target.binding_id.as_str(),
                            target.endpoint_generation,
                        )
                });
                if fenced {
                    historical.outcome = PeerLifecycleOutcome::CleanupOpen;
                }
                return response(historical, None);
            }
            return unavailable(request, "PEER_LIFECYCLE_TARGET_MISSING", true);
        };
        let committed = match exact_target(&state, context, &worker) {
            Ok(target) => target,
            Err(outcome) => {
                let mut failed = result(PeerLifecycleAction::Close, outcome);
                failed.operation_id = Some(operation_id.clone());
                return response(failed, Some("PEER_LIFECYCLE_TARGET_UNPROVEN"));
            }
        };
        if committed != *target {
            return unavailable(request, "PEER_LIFECYCLE_TARGET_MISMATCH", true);
        }
        if target.project_scope != context.project_scope
            || target.app_scope_id != context.app_scope_id
        {
            return unavailable(request, "PEER_LIFECYCLE_SCOPE_MISMATCH", true);
        }
        let snapshot = state::responsibility_snapshot(&state, &target.worker_id);
        if let Some(requires) = close_responsibility_requires(&snapshot) {
            let mut refused = result(PeerLifecycleAction::Close, PeerLifecycleOutcome::Refused);
            refused.operation_id = Some(operation_id.clone());
            refused.requires = requires;
            return response(refused, Some("PEER_LIFECYCLE_RESPONSIBILITY_CONFLICT"));
        }
        let snapshot_captured_ms = target.transport.thread_id.as_deref().and_then(|thread_id| {
            state
                .worker_snapshots
                .get(&target.worker_id)
                .filter(|receipt| receipt.thread_id == thread_id)
                .map(|receipt| receipt.captured_ms)
        });
        let now = now_ms();
        let mut close = close_readback("intent-persisted");
        close.runtime_archive.state = PeerLifecycleStage::Pending;
        (
            state::PeerLifecycleOperationRecord {
                operation_id: operation_id.clone(),
                action: PeerLifecycleAction::Close,
                actor_id: worker_id.clone(),
                project_scope: context.project_scope.clone(),
                app_scope_id: context.app_scope_id.clone(),
                target: Some(target.clone()),
                intent_digest: digest,
                query_capability_hash: capability_hash,
                responsibility_snapshot: Some(snapshot),
                phase: PeerLifecyclePhase::IntentPersisted,
                update: None,
                close: Some(close),
                create: None,
                created_ms: now,
                updated_ms: now,
            },
            snapshot_captured_ms,
        )
    };
    if let Err(error) = commit_record(server, &record) {
        let result = record_result(&record);
        return response(
            result,
            Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
        );
    }
    let dispatched = advance_record_with_close(
        &record,
        PeerLifecyclePhase::HostDispatched,
        None,
        record.close.clone(),
    );
    if let Err(error) = commit_record(server, &dispatched) {
        return response(
            record_result(&dispatched),
            Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
        );
    }
    let host_terminal = archive_terminal(record.exact_target());
    let target_unchanged = {
        let state = server.state.lock().unwrap();
        verify(&state, worker_id, token).is_ok()
            && close_authorized(server, &state, worker_id, &record.exact_target().worker_id)
            && state
                .workers
                .get(&record.exact_target().worker_id)
                .and_then(|worker| exact_target(&state, context, worker).ok())
                .as_ref()
                == Some(record.exact_target())
            && state
                .peer_lifecycle_operations
                .get(operation_id)
                .is_some_and(|stored| stored.phase.is_in_flight())
    };
    if !target_unchanged {
        let mut close = record
            .close
            .clone()
            .unwrap_or_else(|| close_readback("unknown"));
        close.runtime_archive.state = PeerLifecycleStage::Unknown;
        close.worker_retirement.state = PeerLifecycleStage::Unknown;
        close.binding_retirement.state = PeerLifecycleStage::Unknown;
        close.route_retirement.state = PeerLifecycleStage::Unknown;
        close.subscription_retirement.state = PeerLifecycleStage::Unknown;
        close.close_outcome = "unknown".into();
        let terminal =
            advance_record_with_close(&record, PeerLifecyclePhase::Unknown, None, Some(close));
        let _ = commit_record(server, &terminal);
        return response(
            record_result(&terminal),
            Some("PEER_LIFECYCLE_TARGET_CHANGED"),
        );
    }
    let (close, phase, error) = match host_terminal {
        HostTerminal::Verified => {
            let close = retire_control_plane(
                server,
                &record,
                PeerLifecycleStage::Verified,
                None,
                snapshot_captured_ms,
            );
            let phase = close_phase(&close);
            let error = (phase != PeerLifecyclePhase::Complete)
                .then_some("PEER_LIFECYCLE_CONTROL_RETIREMENT_INCOMPLETE".to_owned());
            (close, phase, error)
        }
        HostTerminal::Missing => {
            let close = retire_control_plane(
                server,
                &record,
                PeerLifecycleStage::Missing,
                None,
                snapshot_captured_ms,
            );
            let phase = close_phase(&close);
            let error = (phase != PeerLifecyclePhase::Complete)
                .then_some("PEER_LIFECYCLE_CONTROL_RETIREMENT_INCOMPLETE".to_owned());
            (close, phase, error)
        }
        HostTerminal::Unsupported(detail) => {
            let mut close = close_readback("cleanup-open");
            close.runtime_archive.state = PeerLifecycleStage::Refused;
            close.worker_retirement.state = PeerLifecycleStage::NotAttempted;
            close.binding_retirement.state = PeerLifecycleStage::NotAttempted;
            close.route_retirement.state = PeerLifecycleStage::NotAttempted;
            close.subscription_retirement.state = PeerLifecycleStage::NotAttempted;
            (close, PeerLifecyclePhase::CleanupOpen, Some(detail))
        }
        HostTerminal::Refused(detail) => {
            let mut close = close_readback("refused");
            close.runtime_archive.state = PeerLifecycleStage::Refused;
            close.worker_retirement.state = PeerLifecycleStage::NotAttempted;
            close.binding_retirement.state = PeerLifecycleStage::NotAttempted;
            close.route_retirement.state = PeerLifecycleStage::NotAttempted;
            close.subscription_retirement.state = PeerLifecycleStage::NotAttempted;
            (close, PeerLifecyclePhase::Refused, Some(detail))
        }
        HostTerminal::Unknown(detail) => {
            let mut close = close_readback("unknown");
            close.runtime_archive.state = PeerLifecycleStage::Unknown;
            close.worker_retirement.state = PeerLifecycleStage::NotAttempted;
            close.binding_retirement.state = PeerLifecycleStage::NotAttempted;
            close.route_retirement.state = PeerLifecycleStage::NotAttempted;
            close.subscription_retirement.state = PeerLifecycleStage::NotAttempted;
            (close, PeerLifecyclePhase::Unknown, Some(detail))
        }
    };
    let terminal = advance_record_with_close(&record, phase, None, Some(close));
    if let Err(commit_error) = commit_record(server, &terminal) {
        return response(
            record_result(&terminal),
            Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {commit_error}")),
        );
    }
    response(record_result(&terminal), error.as_deref())
}

fn create_commit(
    server: &Server,
    record: &state::PeerLifecycleOperationRecord,
) -> Result<(), Resp> {
    commit_record(server, record).map_err(|error| {
        // Never present an uncommitted receipt as durable.
        let stored = server
            .state
            .lock()
            .unwrap()
            .peer_lifecycle_operations
            .get(&record.operation_id)
            .cloned();
        response(
            stored.as_ref().map(record_result).unwrap_or_else(|| {
                result(PeerLifecycleAction::Create, PeerLifecycleOutcome::Unknown)
            }),
            Some(&format!("PEER_LIFECYCLE_DURABILITY_FAILED: {error}")),
        )
    })
}

fn create_failure(
    server: &Server,
    mut record: state::PeerLifecycleOperationRecord,
    phase: PeerLifecyclePhase,
    detail: &str,
) -> Resp {
    record.phase = phase;
    record.updated_ms = now_ms();
    let create = record.create.as_mut().expect("Create receipt");
    create.stages.push(detail.to_owned());
    create
        .stages
        .push("cleanup_not_attempted_thread_retained".into());
    create.readiness.state = PeerLifecycleStage::Unknown;
    let cwd = create.cwd.clone();
    if let Some(challenge) = create.readiness.challenge.as_mut() {
        let cleanup = cleanup_challenge_marker(&cwd, &challenge.marker_file);
        challenge.cleanup = Some(if cleanup.is_ok() {
            PeerLifecycleStage::Verified
        } else {
            PeerLifecycleStage::Failed
        });
        if let Err(error) = &cleanup {
            challenge.cleanup_error = Some(error.clone());
        }
        match cleanup {
            Ok(()) => create
                .stages
                .push("challenge_marker_cleanup:verified".into()),
            Err(error) => create
                .stages
                .push(format!("challenge_marker_cleanup:failed:{error}")),
        }
    }
    if let Err(error) = create_commit(server, &record) {
        return error;
    }
    response(record_result(&record), Some(detail))
}
