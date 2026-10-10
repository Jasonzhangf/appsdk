fn parse_wire_request(line: &str) -> Result<(Option<ProjectContext>, Req), String> {
    match serde_json::from_str::<RequestEnvelope>(line) {
        Ok(mut envelope) => {
            envelope
                .normalize_identity_context()
                .map_err(|error| format!("bad request: {error}"))?;
            Ok(envelope.into_parts())
        }
        Err(envelope_error) => {
            // An invalid project_context must not be reinterpreted as a
            // legacy unscoped request merely because serde ignores unknown
            // fields when decoding Req.  That would turn a forged envelope
            // into a context-free Ping or a later default route.
            let has_project_context = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|value| {
                    value
                        .as_object()
                        .map(|object| object.contains_key("project_context"))
                })
                .unwrap_or(false);
            if has_project_context {
                return Err(format!(
                    "bad request: invalid project context envelope: {envelope_error}"
                ));
            }
            serde_json::from_str::<Req>(line)
                .map(|request| (None, request))
                .map_err(|request_error| {
                    format!("bad request: {request_error}; wire envelope parse: {envelope_error}")
                })
        }
    }
}

async fn dispatch_wire(
    server: Arc<Server>,
    project_context: Option<ProjectContext>,
    req: Req,
) -> Resp {
    match req {
        Req::Poll {
            worker_id,
            token,
            timeout_ms,
            receive_id,
        } => {
            let poll_req = Req::Poll {
                worker_id: worker_id.clone(),
                token: token.clone(),
                timeout_ms,
                receive_id: receive_id.clone(),
            };
            if let Err(error) =
                validate_request_context(&server, &poll_req, project_context.as_ref())
            {
                return Resp::err(error);
            }
            let admission = {
                let check = server.state.lock().unwrap();
                if let Err(error) = verify(&check, &worker_id, &token) {
                    Some(error)
                } else if check.admission_frozen() {
                    Some(Resp::err("MIGRATION_ADMISSION_FROZEN: only identity rebind, read queries, daemon restart, and migration verify are allowed"))
                } else {
                    None
                }
            };
            match admission {
                Some(response) => response,
                None => {
                    handle_poll_async_with_context(
                        server,
                        worker_id,
                        Some(token),
                        timeout_ms,
                        project_context,
                        receive_id,
                    )
                    .await
                }
            }
        }
        req => tokio::task::spawn_blocking(move || {
            let route_gate = if mutation_blocked_during_migration(&req)
                || wire_mutation_principal(&req).is_some()
            {
                Some(wire_route_mutation_gate(&server))
            } else {
                None
            };
            let _route_gate_guard = route_gate.as_ref().map(|gate| gate.lock().unwrap());
            if let Err(error) = validate_request_context(&server, &req, project_context.as_ref()) {
                return Resp::err(error);
            }
            dispatch_with_route_context(&server, req, project_context)
        })
        .await
        .unwrap_or_else(|e| Resp::err(format!("handler join error: {}", e))),
    }
}

/// Route a host-daemon connection to the reducer that owns its exact
/// `(app_scope_id, project_scope_id)` pair.  Registration is handled inside a
/// blocking transaction so the route table is published only after the target
/// journal has committed the identity binding.
async fn dispatch_wire_routed(
    manager: Arc<ProjectRuntimeManager>,
    project_context: Option<ProjectContext>,
    req: Req,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> (Arc<Server>, Resp) {
    match req {
        Req::RouteResolve { tmux_endpoint } => {
            let response = match manager.resolve_route_by_tmux_endpoint(&tmux_endpoint) {
                Ok(route) => match serde_json::to_value(route) {
                    Ok(value) => Resp::data(value),
                    Err(error) => {
                        Resp::err(format!("ROUTE_RESOLVE_INVALID: serialize route: {error}"))
                    }
                },
                Err(error) => Resp::err(error),
            };
            (manager.host.clone(), response)
        }
        Req::RouteResolvePaneRecovery {
            tmux_endpoint,
            worker_id,
            token,
        } => {
            let response =
                match manager.resolve_staged_pane_recovery(&tmux_endpoint, &worker_id, &token) {
                    Ok(route) => match serde_json::to_value(route) {
                        Ok(value) => Resp::data(value),
                        Err(error) => {
                            Resp::err(format!("ROUTE_RESOLVE_INVALID: serialize route: {error}"))
                        }
                    },
                    Err(error) => Resp::err(error),
                };
            (manager.host.clone(), response)
        }
        Req::RouteResolveNative {
            session_id,
            native_thread_id,
        } => {
            let response =
                match manager.resolve_route_by_native_thread(&session_id, &native_thread_id) {
                    Ok(route) => match serde_json::to_value(route) {
                        Ok(value) => Resp::data(value),
                        Err(error) => {
                            Resp::err(format!("ROUTE_RESOLVE_INVALID: serialize route: {error}"))
                        }
                    },
                    Err(error) => Resp::err(error),
                };
            (manager.host.clone(), response)
        }
        Req::Poll {
            worker_id,
            token,
            timeout_ms,
            receive_id,
        } => {
            let Some(context) = project_context else {
                return (
                    manager.host.clone(),
                    Resp::err(
                        "PROJECT_CONTEXT_REQUIRED: canonical project root and scope are required",
                    ),
                );
            };
            let poll_req = Req::Poll {
                worker_id: worker_id.clone(),
                token: token.clone(),
                timeout_ms,
                receive_id: receive_id.clone(),
            };
            let server = match manager.select_runtime(&context) {
                Ok(server) => server,
                Err(error) => return (manager.host.clone(), Resp::err(error)),
            };
            if let Err(error) = validate_request_context(&server, &poll_req, Some(&context)) {
                return (server, Resp::err(error));
            }
            let admission = {
                let check = server.state.lock().unwrap();
                if let Err(error) = verify(&check, &worker_id, &token) {
                    Some(error)
                } else if check.admission_frozen() {
                    Some(Resp::err("MIGRATION_ADMISSION_FROZEN: only identity rebind, read queries, daemon restart, and migration verify are allowed"))
                } else {
                    None
                }
            };
            let response = match admission {
                Some(response) => response,
                None => {
                    let poll = handle_poll_async_with_context(
                        server.clone(),
                        worker_id,
                        Some(token),
                        timeout_ms,
                        Some(context),
                        receive_id,
                    );
                    tokio::pin!(poll);
                    tokio::select! {
                        response = poll.as_mut() => response,
                        _ = shutdown.changed() => Resp::err("DAEMON_SHUTTING_DOWN"),
                    }
                }
            };
            (server, response)
        }
        req => {
            let result = tokio::task::spawn_blocking({
                let manager = manager.clone();
                move || manager.dispatch_sync(project_context, req)
            })
            .await;
            match result {
                Ok(result) => result,
                Err(error) => (
                    manager.host.clone(),
                    Resp::err(format!("handler join error: {error}")),
                ),
            }
        }
    }
}

async fn conn_task(server: Arc<Server>, stream: tokio::net::UnixStream) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let resp = match parse_wire_request(&line) {
            Ok((project_context, req)) => {
                let activity_req = req.clone();
                let resp = dispatch_wire(server.clone(), project_context, req).await;
                let _ = record_activity(
                    &server.storage_root,
                    "request",
                    request_activity(&activity_req, &resp),
                );
                resp
            }
            Err(error) => {
                let resp = Resp::err(error);
                let _ = record_activity(
                    &server.storage_root,
                    "protocol_error",
                    json!({"error": resp.error}),
                );
                resp
            }
        };
        let mut out = serde_json::to_string(&resp).expect("serialize resp");
        out.push('\n');
        if writer.write_all(out.as_bytes()).await.is_err() {
            break;
        }
    }
}

async fn conn_task_routed(
    manager: Arc<ProjectRuntimeManager>,
    stream: tokio::net::UnixStream,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    loop {
        let line = tokio::select! {
            _ = shutdown.changed() => break,
            line = lines.next_line() => match line {
                Ok(Some(line)) => line,
                _ => break,
            },
        };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match parse_wire_request(&line) {
            Ok((project_context, req)) => {
                let query_activity = is_identity_query_intent(&req);
                let activity_req = req.clone();
                let (runtime, resp) =
                    dispatch_wire_routed(manager.clone(), project_context, req, shutdown.clone())
                        .await;
                if !query_activity {
                    let _ = record_activity(
                        &runtime.storage_root,
                        "request",
                        request_activity(&activity_req, &resp),
                    );
                }
                resp
            }
            Err(error) => {
                let resp = Resp::err(error);
                let _ = record_activity(
                    &manager.host.storage_root,
                    "protocol_error",
                    json!({"error": resp.error}),
                );
                resp
            }
        };
        let mut out = serde_json::to_string(&resp).expect("serialize resp");
        out.push('\n');
        if writer.write_all(out.as_bytes()).await.is_err() {
            break;
        }
    }
}

fn is_identity_query_intent(request: &Req) -> bool {
    matches!(
        request,
        Req::IdentityContext {
            identity_context: Some(identity_context),
            ..
        } if is_identity_query_request(identity_context)
    )
}

fn is_identity_query_request(request: &IdentityContextRequest) -> bool {
    request.query || request.invocation == "query" || request.action == "query"
}

fn is_valid_identity_query_shape(request: &IdentityContextRequest) -> bool {
    request.query && request.invocation == "query" && request.action == "query"
}

async fn conn_task_degraded(
    operation_journal: Arc<crate::server::operation_journal::OperationJournal>,
    project_replay_failure: Arc<str>,
    stream: tokio::net::UnixStream,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let resp = match parse_wire_request(&line) {
            Ok((_, Req::Ping)) => Resp::data(json!({
                "degraded": true,
                "now": iso(now_ms()),
            })),
            Ok((
                Some(project_context),
                Req::IdentityContext {
                    facts,
                    identity_context,
                },
            )) => {
                let request =
                    identity_context.unwrap_or_else(|| IdentityContextRequest::legacy(facts));
                if is_identity_query_request(&request) {
                    if !is_valid_identity_query_shape(&request) {
                        Resp::err("IDENTITY_OPERATION_QUERY_SHAPE_INVALID")
                    } else {
                        match operation_journal.query(
                            &request,
                            project_context.project_scope.as_str(),
                            project_context.app_scope_id.as_str(),
                        ) {
                            Ok(result) => {
                                let mut result =
                                    serde_json::to_value(result).unwrap_or(serde_json::Value::Null);
                                if let Some(projection) =
                                    result.get_mut("result").and_then(|value| value.as_object_mut())
                                {
                                    if projection.contains_key("nested_command_id")
                                        || projection.contains_key("nested_operation_id")
                                    {
                                        projection.insert(
                                            "nested_receipt".into(),
                                            json!({
                                                "state": "unavailable",
                                                "error": format!(
                                                    "PROJECT_OWNER_READBACK_UNAVAILABLE: {project_replay_failure}"
                                                ),
                                            }),
                                        );
                                    }
                                }
                                Resp::data(result)
                            }
                            Err(error) => Resp::err(error),
                        }
                    }
                } else {
                    Resp::err("PROJECT_RUNTIME_UNAVAILABLE: project replay failed")
                }
            }
            Ok(_) => Resp::err("PROJECT_RUNTIME_UNAVAILABLE: project replay failed"),
            Err(error) => Resp::err(error),
        };
        let mut out = serde_json::to_string(&resp).expect("serialize resp");
        out.push('\n');
        if writer.write_all(out.as_bytes()).await.is_err() {
            break;
        }
    }
}

fn apply_replayed_event(st: &mut State, event: &Event, line: usize) -> anyhow::Result<()> {
    st.apply_checked(event)
        .map_err(|error| anyhow::anyhow!("journal replay failed at line {line}: {error}"))?;
    match event {
        Event::ReducerCheckpoint { sequence, revision } => st
            .set_checkpoint_version(*sequence, *revision)
            .map_err(|error| anyhow::anyhow!("journal replay failed at line {line}: {error}"))
            .and_then(|_| {
                st.global.validate().map_err(|error| {
                    anyhow::anyhow!(
                        "journal replay failed at line {line}: global state validation: {error}"
                    )
                })
            }),
        _ => st
            .advance_version()
            .map_err(|error| anyhow::anyhow!("journal replay failed at line {line}: {error}")),
    }
}

fn apply_replayed_snapshot_event(st: &mut State, event: &Event, line: usize) -> anyhow::Result<()> {
    match event {
        Event::ReducerSnapshot { .. } | Event::ReducerCheckpoint { .. } => {
            anyhow::bail!(
                "journal replay failed at line {line}: reducer snapshot contains metadata record"
            );
        }
        _ => st
            .apply_checked(event)
            .map_err(|error| anyhow::anyhow!("journal replay failed at line {line}: {error}")),
    }
}

fn validate_snapshot_command_frames(
    events: &[Event],
    line: usize,
    seen_command_ids: &mut std::collections::HashSet<String>,
    seen_operation_ids: &mut std::collections::HashMap<String, String>,
) -> anyhow::Result<()> {
    let mut pending_command: Option<(String, String)> = None;
    for event in events {
        if let Event::CommandStarted {
            command_id,
            operation_id,
        } = event
        {
            if !seen_command_ids.insert(command_id.clone()) {
                anyhow::bail!(
                    "journal replay failed at line {line}: duplicate command {command_id}"
                );
            }
            if let Some(existing_command_id) =
                seen_operation_ids.insert(operation_id.clone(), command_id.clone())
            {
                anyhow::bail!(
                    "journal replay failed at line {line}: operation {operation_id} already belongs to command {existing_command_id}"
                );
            }
            if pending_command.is_some() {
                anyhow::bail!("journal replay failed at line {line}: nested command {command_id}");
            }
            pending_command = Some((command_id.clone(), operation_id.clone()));
            continue;
        }
        if let Some((command_id, operation_id)) = pending_command.as_ref() {
            if let Event::CommandCompleted {
                command_id: completed_id,
                operation_id: completed_operation,
                receipt,
            } = event
            {
                if completed_id != command_id || completed_operation != operation_id {
                    anyhow::bail!(
                        "journal replay failed at line {line}: command completion does not match start"
                    );
                }
                if receipt.operation_id != *operation_id {
                    anyhow::bail!(
                        "journal replay failed at line {line}: command receipt operation does not match start"
                    );
                }
                pending_command = None;
            }
            continue;
        }
        if matches!(event, Event::CommandCompleted { .. }) {
            anyhow::bail!("journal replay failed at line {line}: command completion without start");
        }
    }
    if let Some((command_id, _)) = pending_command {
        anyhow::bail!(
            "journal replay failed: incomplete command {command_id}; completion marker missing"
        );
    }
    Ok(())
}

fn track_legacy_master_authority(
    event: &Event,
    legacy_master_is_current: &mut bool,
    saw_typed_master_authority: &mut bool,
) {
    match event {
        Event::MasterAssigned { .. } => {
            *legacy_master_is_current = !*saw_typed_master_authority;
        }
        Event::GlobalRuntimeBound { .. } => {
            *legacy_master_is_current = false;
        }
        Event::GlobalMasterGranted { .. } | Event::GlobalMasterRevoked { .. } => {
            *legacy_master_is_current = false;
            *saw_typed_master_authority = true;
        }
        _ => {}
    }
}

fn replay(root: &Path) -> anyhow::Result<State> {
    replay_from_journal(root, &root.join(".agent-collab/server/journal.jsonl"))
}

/// Replay the host index exactly as the daemon does.
///
/// The offline reset owner must use this reader and not a private one. The
/// claim it retires has to be the same binding the reducer compares against on
/// the next start, and only this reader performs the same current-thread
/// restore and legacy indexing steps.
pub fn replay_host_index(storage_root: &Path) -> anyhow::Result<State> {
    let storage_root = std::fs::canonicalize(storage_root)?;
    replay_from_journal(
        &storage_root,
        &storage_root.join(".agent-collab/server/journal.jsonl"),
    )
}

/// Replay a project reducer from the journal selected by its runtime owner.
/// The project root remains the semantic scope used by worktree and identity
/// validation; the journal path may be an appserver-specific runtime store.
fn replay_from_journal(root: &Path, journal: &Path) -> anyhow::Result<State> {
    let mut st = State::default();
    if !journal.exists() {
        return Ok(st);
    }
    let content = std::fs::read_to_string(&journal)?;
    let mut events = Vec::new();
    let mut pending_command: Option<(String, String, Vec<Event>)> = None;
    let mut seen_command_ids = std::collections::HashSet::new();
    let mut seen_operation_ids = std::collections::HashMap::new();
    let mut convert_root = false;
    let mut saw_current_thread_route = false;
    let mut legacy_master_is_current = false;
    let mut saw_typed_master_authority = false;
    let mut saw_snapshot_baseline = false;
    let mut saw_real_event = false;
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let line_events = decode_journal_line(trimmed).map_err(|error| {
            anyhow::anyhow!(
                "journal replay failed at line {}: {}; manual journal edits are unsupported",
                index + 1,
                error
            )
        })?;
        if line_events.len() > 1
            && !line_events
                .iter()
                .any(|event| matches!(event, Event::ReducerCheckpoint { .. }))
        {
            convert_root = true;
        }
        for event in line_events {
            if let Event::ReducerSnapshot {
                sequence,
                revision,
                events: snapshot_events,
            } = &event
            {
                if saw_snapshot_baseline {
                    anyhow::bail!(
                        "journal replay failed at line {}: duplicate reducer snapshot",
                        index + 1
                    );
                }
                if saw_real_event || saw_current_thread_route {
                    anyhow::bail!(
                        "journal replay failed at line {}: reducer snapshot is not the initial baseline",
                        index + 1
                    );
                }
                if pending_command.is_some() {
                    anyhow::bail!(
                        "journal replay failed at line {}: reducer snapshot nested in command",
                        index + 1
                    );
                }
                saw_snapshot_baseline = true;
                validate_snapshot_command_frames(
                    snapshot_events,
                    index + 1,
                    &mut seen_command_ids,
                    &mut seen_operation_ids,
                )?;
                for nested in snapshot_events {
                    if matches!(nested, Event::ReducerSnapshot { .. })
                        || matches!(nested, Event::ReducerCheckpoint { .. })
                    {
                        anyhow::bail!(
                            "journal replay failed at line {}: reducer snapshot contains metadata record",
                            index + 1
                        );
                    }
                    if matches!(nested, Event::GlobalCurrentThreadRouteSet { .. }) {
                        saw_current_thread_route = true;
                    }
                    apply_replayed_snapshot_event(&mut st, nested, index + 1)?;
                    track_legacy_master_authority(
                        nested,
                        &mut legacy_master_is_current,
                        &mut saw_typed_master_authority,
                    );
                    events.push(nested.clone());
                }
                st.set_checkpoint_version(*sequence, *revision)
                    .map_err(|error| {
                        anyhow::anyhow!("journal replay failed at line {}: {error}", index + 1)
                    })?;
                continue;
            }
            saw_real_event = true;
            if matches!(event, Event::GlobalCurrentThreadRouteSet { .. }) {
                saw_current_thread_route = true;
            }
            if let Event::CommandStarted {
                command_id,
                operation_id,
            } = &event
            {
                if !seen_command_ids.insert(command_id.clone()) {
                    anyhow::bail!(
                        "journal replay failed at line {}: duplicate command {}",
                        index + 1,
                        command_id
                    );
                }
                if let Some(existing_command_id) =
                    seen_operation_ids.insert(operation_id.clone(), command_id.clone())
                {
                    anyhow::bail!(
                        "journal replay failed at line {}: operation {} already belongs to command {}",
                        index + 1,
                        operation_id,
                        existing_command_id
                    );
                }
                if pending_command.is_some() {
                    anyhow::bail!(
                        "journal replay failed at line {}: nested command {}",
                        index + 1,
                        command_id
                    );
                }
                pending_command = Some((
                    command_id.clone(),
                    operation_id.clone(),
                    vec![event.clone()],
                ));
                continue;
            }
            if let Some((command_id, operation_id, pending)) = pending_command.as_mut() {
                match &event {
                    Event::CommandCompleted {
                        command_id: completed_id,
                        operation_id: completed_operation,
                        receipt,
                    } => {
                        if completed_id != command_id || completed_operation != operation_id {
                            anyhow::bail!(
                                "journal replay failed at line {}: command completion does not match start",
                                index + 1
                            );
                        }
                        if receipt.operation_id != *operation_id {
                            anyhow::bail!(
                                "journal replay failed at line {}: command receipt operation does not match start",
                                index + 1
                            );
                        }
                        pending.push(event.clone());
                        let committed = std::mem::take(pending);
                        pending_command = None;
                        for event in committed {
                            apply_replayed_event(&mut st, &event, index + 1)?;
                            track_legacy_master_authority(
                                &event,
                                &mut legacy_master_is_current,
                                &mut saw_typed_master_authority,
                            );
                            events.push(event);
                        }
                    }
                    Event::CommandStarted { .. } => unreachable!(),
                    _ => pending.push(event.clone()),
                }
                continue;
            }
            if matches!(event, Event::CommandCompleted { .. }) {
                anyhow::bail!(
                    "journal replay failed at line {}: command completion without start",
                    index + 1
                );
            }
            if matches!(event, Event::MasterAssigned { .. })
                && (line.contains("\"ev\":\"RootAssigned\"")
                    || line.contains("\"ev\": \"RootAssigned\""))
            {
                convert_root = true;
            }
            apply_replayed_event(&mut st, &event, index + 1)?;
            track_legacy_master_authority(
                &event,
                &mut legacy_master_is_current,
                &mut saw_typed_master_authority,
            );
            events.push(event);
        }
    }
    if let Some((command_id, _, _)) = pending_command {
        anyhow::bail!(
            "journal replay failed: incomplete command {command_id}; completion marker missing"
        );
    }
    if !saw_current_thread_route {
        st.restore_unique_current_thread_routes_from_bindings()
            .map_err(|error| anyhow::anyhow!("journal replay failed: {error}"))?;
    }
    // Always index durable thread-only bindings, whether or not this journal
    // carried a strict route event.  A journal whose route events are all
    // thread-only (the live host journal) must still expose them after a
    // restart, and the strict-route guard above would otherwise skip them.
    st.index_legacy_thread_routes_from_bindings()
        .map_err(|error| anyhow::anyhow!("journal replay failed: {error}"))?;
    if legacy_master_is_current {
        let Some(worker_id) = st.master_worker_id.clone() else {
            unreachable!("legacy master event must leave a legacy master projection");
        };
        if let Some(route_scope) = route_scope_for_root(root, &st)
            .map_err(|error| anyhow::anyhow!("journal replay failed: {error}"))?
        {
            let has_typed_grant = st
                .global
                .lookup_project_for_route(&route_scope)
                .is_some_and(|project| {
                    project.master_grants.values().any(|grant| {
                        grant.project_scope == route_scope.project_scope_id
                            && grant.app_scope_id == route_scope.app_scope_id
                            && grant.agent_id.as_str() == worker_id
                    })
                });
            if !has_typed_grant {
                let grant = master_grant_for_worker(
                    &st,
                    &route_scope,
                    &worker_id,
                    st.master_assigned_by.as_deref().unwrap_or(&worker_id),
                    st.master_approval
                        .as_deref()
                        .unwrap_or("legacy master assignment imported"),
                )
                .map_err(|error| {
                    anyhow::anyhow!(
                        "journal replay failed: legacy master assignment for {worker_id} cannot be migrated: {error}"
                    )
                })?;
                st.global
                    .grant_master(grant)
                    .map_err(|error| anyhow::anyhow!("journal replay failed: {error}"))?;
            }
            st.master_worker_id = None;
            st.master_assigned_by = None;
            st.master_approval = None;
            st.master_assigned_ms = None;
        }
    } else {
        st.master_worker_id = None;
        st.master_assigned_by = None;
        st.master_approval = None;
        st.master_assigned_ms = None;
    }
    st.global.validate().map_err(|error| {
        anyhow::anyhow!("journal replay failed: global state validation: {error}")
    })?;
    if convert_root {
        let mut body = String::new();
        for event in events {
            body.push_str(&serde_json::to_string(&event)?);
            body.push('\n');
        }
        let tmp = journal.with_file_name("journal.jsonl.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &journal)?;
    }
    Ok(st)
}

fn decode_journal_line(line: &str) -> Result<Vec<Event>, notification_contract::JournalError> {
    let mut events = Vec::new();
    let mut stream = serde_json::Deserializer::from_str(line).into_iter::<Event>();
    while let Some(item) = stream.next() {
        events.push(
            item.map_err(|error| notification_contract::JournalError::Replay(error.to_string()))?,
        );
    }
    let offset = stream.byte_offset();
    if offset < line.len() && !line[offset..].trim().is_empty() {
        return Err(notification_contract::JournalError::Replay(
            "trailing characters".into(),
        ));
    }
    if events.is_empty() {
        return Err(notification_contract::JournalError::Replay(
            "empty event".into(),
        ));
    }
    Ok(events)
}

fn acquire_daemon_lock(lock_path: &Path, socket_path: &Path) -> anyhow::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)?;
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => anyhow::bail!(
            "server already running at {}: {}",
            socket_path.display(),
            error
        ),
        Some(code) if code == libc::EPERM => anyhow::bail!(
            "cannot acquire daemon lock at {}: flock is unavailable or denied; refusing PID fallback: {}",
            lock_path.display(),
            error
        ),
        _ => Err(error.into()),
    }
}

/// Acquire the same host writer lock used by the resident daemon. Offline
/// maintenance commands use this to prove no daemon writer exists before they
/// mutate durable state.
pub(crate) fn acquire_reset_lock(
    lock_path: &Path,
    socket_path: &Path,
) -> anyhow::Result<std::fs::File> {
    acquire_daemon_lock(lock_path, socket_path)
}

fn acquire_legacy_writer_lock(
    lock_path: &Path,
    description: &str,
) -> anyhow::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(lock_path)?;
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
    ) {
        anyhow::bail!(
            "DAEMON_MIGRATION_REQUIRED: legacy {description} lock is held at {}; stop or migrate the legacy writer before starting the host daemon",
            lock_path.display()
        );
    }
    Err(error.into())
}

fn legacy_host_daemon_lock_path(host_paths: &HostPaths) -> PathBuf {
    legacy_host_daemon_lock_path_for(host_paths.state_root(), &default_state_root())
}

fn legacy_host_daemon_lock_path_for(state_root: &Path, default_root: &Path) -> PathBuf {
    if state_root == default_root {
        PathBuf::from(LEGACY_HOST_DAEMON_LOCK_PATH)
    } else {
        state_root.join("legacy-host.lock")
    }
}

fn default_state_root() -> PathBuf {
    std::env::var_os(crate::scope::HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".collab"))
        .unwrap_or_else(|| PathBuf::from("/nonexistent/collab-state"))
}

fn probe_legacy_socket(socket_path: &Path) -> anyhow::Result<bool> {
    match crate::client::connect(socket_path) {
        Ok(stream) => {
            drop(stream);
            Ok(true)
        }
        Err(error) if crate::client::stale_socket_error(&error) => Ok(false),
        Err(error) => Err(anyhow::Error::new(error)),
    }
}

pub(crate) struct LegacyWriterFence {
    // These guards intentionally stay alive for the complete daemon lifetime.
    // The legacy binary does not know the new host lock, so a one-shot probe is
    // insufficient to prevent it from opening the same journal after startup.
    _host_lock: Option<std::fs::File>,
    _project_lock: Option<std::fs::File>,
}

/// Reject a reachable pre-host-endpoint socket after all compatible legacy
/// locks are held. A stale socket is safe to classify as absent only for the
/// platform's explicit stale-socket errors; any other probe error is unknown
/// and fails closed.
fn ensure_legacy_socket_absent(scope: &Scope, host_paths: &HostPaths) -> anyhow::Result<()> {
    let project_server_dir = scope.server_dir();
    let legacy_socket = project_server_dir.join("server.sock");
    if legacy_socket != host_paths.socket_path() && probe_legacy_socket(&legacy_socket)? {
        anyhow::bail!(
            "DAEMON_MIGRATION_REQUIRED: legacy project daemon is reachable at {}; stop or migrate it before starting the host daemon",
            legacy_socket.display()
        );
    }
    Ok(())
}

/// Hold every lock understood by the pre-host-endpoint daemon until the new
/// daemon exits.  Acquiring these locks before replay/journal open closes the
/// check-to-open race: an old writer can neither start after the check nor
/// acquire the same project lock while this process owns the journal.
pub(crate) fn acquire_legacy_writer_fence(
    scope: &Scope,
    host_paths: &HostPaths,
) -> anyhow::Result<LegacyWriterFence> {
    let legacy_host_lock = legacy_host_daemon_lock_path(host_paths);
    let project_server_dir = scope.server_dir();
    let host_lock = if legacy_host_lock != host_paths.lock_path() {
        Some(acquire_legacy_writer_lock(
            &legacy_host_lock,
            "host daemon",
        )?)
    } else {
        None
    };

    let legacy_project_lock = project_server_dir.join("daemon.lock");
    let project_lock = if legacy_project_lock != host_paths.lock_path()
        && legacy_project_lock != legacy_host_lock
        && project_server_dir.is_dir()
    {
        Some(acquire_legacy_writer_lock(
            &legacy_project_lock,
            "project daemon",
        )?)
    } else {
        None
    };

    ensure_legacy_socket_absent(scope, host_paths)?;
    Ok(LegacyWriterFence {
        _host_lock: host_lock,
        _project_lock: project_lock,
    })
}

fn is_provisional_cli_runtime(project_context: &ProjectContext, worker_id: &str) -> bool {
    project_context
        .runtime_context
        .as_ref()
        .is_some_and(|runtime| {
            crate::identity::RuntimeIdentity::cli_adapter(worker_id)
                .is_ok_and(|provisional| runtime == &provisional)
        })
}

fn same_inode(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn prepare_socket_path(sock_path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::FileTypeExt;

    let before = match std::fs::symlink_metadata(sock_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !before.file_type().is_socket() {
        anyhow::bail!(
            "server socket path is occupied by {}; refusing to remove it",
            sock_path.display()
        );
    }
    match crate::client::connect(sock_path) {
        Ok(_) => anyhow::bail!("server already running at {}", sock_path.display()),
        Err(error) if crate::client::stale_socket_error(&error) => {}
        Err(error) => {
            return Err(anyhow::Error::new(error).context(format!(
                "cannot determine whether stale server socket {} can be removed",
                sock_path.display()
            )))
        }
    }
    let after = match std::fs::symlink_metadata(sock_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !same_inode(&before, &after) {
        anyhow::bail!(
            "server socket changed while checking {}; refusing to remove it",
            sock_path.display()
        );
    }
    std::fs::remove_file(sock_path)?;
    Ok(())
}

fn remove_listener_socket(sock_path: &Path, captured: &std::fs::Metadata) -> anyhow::Result<()> {
    let current = match std::fs::symlink_metadata(sock_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if same_inode(captured, &current) {
        std::fs::remove_file(sock_path)?;
    }
    Ok(())
}

pub async fn run(scope: Scope) -> anyhow::Result<()> {
    let host_paths = scope.host_paths()?;
    run_with_host_paths(scope, host_paths).await
}

async fn run_timer_scheduler<T, R, F>(
    mut stop_rx: tokio::sync::mpsc::Receiver<()>,
    tick_interval_ms: u64,
    runtimes: R,
    spawn_tick: F,
) where
    T: Send + 'static,
    R: Fn() -> Vec<T> + Send + 'static,
    F: Fn(Vec<T>) -> tokio::task::JoinHandle<()> + Send + 'static,
{
    let mut interval = tokio::time::interval(Duration::from_millis(tick_interval_ms));
    let mut tick = None;
    loop {
        tokio::select! {
            _ = stop_rx.recv() => break,
            _ = interval.tick() => {
                if tick.as_ref().is_some_and(|task: &tokio::task::JoinHandle<()>| !task.is_finished()) {
                    continue;
                }
                if let Some(completed) = tick.take() {
                    let _ = completed.await;
                }
                tick = Some(spawn_tick(runtimes()));
            }
        }
    }
    if let Some(tick) = tick {
        tick.abort();
        let _ = tick.await;
    }
}

async fn run_with_host_paths(scope: Scope, host_paths: HostPaths) -> anyhow::Result<()> {
    host_paths.ensure_root()?;
    let sock_path = host_paths.socket_path();
    let project_server_dir = scope.server_dir();
    std::fs::create_dir_all(&project_server_dir)?;

    // The host lock is the only writable daemon admission gate.  Project
    // roots still select their own journal/reducer storage, but never another
    // socket or a second host writer.
    let _lock_file = acquire_daemon_lock(&host_paths.lock_path(), &sock_path)?;

    // Keep every legacy writer lock through the journal writer lifetime. If
    // the compatibility fixture uses the legacy path as its host path, the
    // normal duplicate-daemon error remains authoritative; a real host-path
    // migration reaches this persistent fence because its locks are distinct.
    let _legacy_writer_fence = acquire_legacy_writer_fence(&scope, &host_paths)?;

    prepare_socket_path(&sock_path)?;

    let operation_journal = Arc::new(
        crate::server::operation_journal::OperationJournal::open(host_paths.journal_path())
            .map_err(|error| anyhow::anyhow!("OPERATION_JOURNAL_REPLAY_FAILED: {error}"))?,
    );
    let state = match replay(&scope.root) {
        Ok(state) => state,
        Err(error) => {
            let project_replay_failure = format!("{error:#}");
            append_log(
                &host_paths.log_path(),
                &format!("project replay failed; entering query-only degraded mode: {project_replay_failure}"),
            );
            let listener = UnixListener::bind(&sock_path)?;
            let socket_metadata = std::fs::symlink_metadata(&sock_path)?;
            use std::os::unix::fs::PermissionsExt;
            if let Err(error) =
                std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))
            {
                let cleanup = remove_listener_socket(&sock_path, &socket_metadata);
                return match cleanup {
                    Ok(()) => Err(error.into()),
                    Err(cleanup_error) => Err(anyhow::Error::new(error).context(format!(
                        "failed to clean up startup socket: {cleanup_error}"
                    ))),
                };
            }
            let pid_path = host_paths.pid_path();
            std::fs::write(&pid_path, std::process::id().to_string()).map_err(|error| {
                match remove_listener_socket(&sock_path, &socket_metadata) {
                    Ok(()) => anyhow::Error::new(error),
                    Err(cleanup_error) => anyhow::Error::new(error).context(format!(
                        "failed to clean up startup socket: {cleanup_error}"
                    )),
                }
            })?;
            let pid_metadata = std::fs::symlink_metadata(&pid_path)?;
            let state_root_metadata = std::fs::symlink_metadata(host_paths.state_root())?;
            let mut interrupt = Box::pin(tokio::signal::ctrl_c());
            let mut terminate = Box::pin(async {
                let mut signal =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
                signal.recv().await;
                Ok::<(), std::io::Error>(())
            });
            let mut state_root_removed = Box::pin(async {
                let mut interval = tokio::time::interval(Duration::from_millis(250));
                interval.tick().await;
                loop {
                    interval.tick().await;
                    if !std::fs::symlink_metadata(host_paths.state_root())
                        .is_ok_and(|current| same_inode(&state_root_metadata, &current))
                    {
                        return;
                    }
                }
            });
            let mut connection_tasks = tokio::task::JoinSet::new();
            tokio::select! {
                _ = interrupt.as_mut() => {}
                _ = terminate.as_mut() => {}
                _ = state_root_removed.as_mut() => {}
                result = async {
                    loop {
                        match listener.accept().await {
                            Ok((stream, _)) => {
                                connection_tasks.spawn(conn_task_degraded(
                                    operation_journal.clone(),
                                    Arc::from(project_replay_failure.as_str()),
                                    stream,
                                ));
                            }
                            Err(error) => append_log(
                                &host_paths.log_path(),
                                &format!("accept error: {}", error),
                            ),
                        }
                    }
                } => result,
            }
            while connection_tasks.join_next().await.is_some() {}
            let _ = remove_listener_socket(&sock_path, &socket_metadata);
            if std::fs::symlink_metadata(&pid_path)
                .is_ok_and(|current| same_inode(&pid_metadata, &current))
            {
                let _ = std::fs::remove_file(&pid_path);
            }
            return Ok(());
        }
    };
    let journal_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(project_server_dir.join("journal.jsonl"))?;

    append_log(&host_paths.log_path(), "server starting");

    let server = Arc::new(Server {
        config: crate::config::load(&scope.root)?,
        root: scope.root.clone(),
        storage_root: scope.root.clone(),
        journal_path: project_server_dir.join("journal.jsonl"),
        host_paths: host_paths.clone(),
        state: Mutex::new(state),
        journal: Mutex::new(journal_file),
        appserver_candidate_check: default_appserver_candidate_check(),
        appserver_notification_sink: default_appserver_notification_sink(),
        appserver_thread_status: default_appserver_thread_status(),
        appserver_thread_archive: default_appserver_thread_archive(),
        #[cfg(not(test))]
        tmux_notification_sink: default_tmux_notification_sink(),
        mailbox_notify: Notify::new(),
    });
    restore_registered_peer_default_leases(&server);
    purge_expired_storage(&server, now_ms());
    let runtime_manager = ProjectRuntimeManager::new_with_operation_journal(
        server.clone(),
        &host_paths,
        operation_journal,
    )
    .map_err(|error| anyhow::anyhow!(error))?;
    let listener = UnixListener::bind(&sock_path)?;
    let socket_metadata = std::fs::symlink_metadata(&sock_path)?;
    use std::os::unix::fs::PermissionsExt;
    if let Err(error) = std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))
    {
        let cleanup = remove_listener_socket(&sock_path, &socket_metadata);
        return match cleanup {
            Ok(()) => Err(error.into()),
            Err(cleanup_error) => Err(anyhow::Error::new(error).context(format!(
                "failed to clean up startup socket: {cleanup_error}"
            ))),
        };
    }
    let pid_path = host_paths.pid_path();
    std::fs::write(&pid_path, std::process::id().to_string()).map_err(|error| {
        match remove_listener_socket(&sock_path, &socket_metadata) {
            Ok(()) => anyhow::Error::new(error),
            Err(cleanup_error) => anyhow::Error::new(error).context(format!(
                "failed to clean up startup socket: {cleanup_error}"
            )),
        }
    })?;
    let pid_metadata = std::fs::symlink_metadata(&pid_path)?;
    let state_root_metadata = std::fs::symlink_metadata(host_paths.state_root())?;
    let _ = record_activity(
        &scope.root,
        "daemon_start",
        json!({"pid": std::process::id()}),
    );

    let mut interrupt = Box::pin(tokio::signal::ctrl_c());
    let mut terminate = Box::pin(async {
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        signal.recv().await;
        Ok::<(), std::io::Error>(())
    });
    let mut state_root_removed = Box::pin(async {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.tick().await;
        loop {
            interval.tick().await;
            if !std::fs::symlink_metadata(host_paths.state_root())
                .is_ok_and(|current| same_inode(&state_root_metadata, &current))
            {
                return;
            }
        }
    });

    // Background scheduler: bounded waits and explicitly registered notifications.
    let sched = runtime_manager.clone();
    let (scheduler_stop, scheduler_stop_rx) = tokio::sync::mpsc::channel::<()>(1);
    let scheduler_interval_ms = sched.host.config.timers.tick_interval_ms;
    let scheduler = tokio::spawn(run_timer_scheduler(
        scheduler_stop_rx,
        scheduler_interval_ms,
        move || sched.runtimes(),
        |runtimes| {
            tokio::task::spawn_blocking(move || {
                for runtime in runtimes {
                    crate::server::timers::tick(&runtime);
                }
            })
        },
    ));

    let mut connection_tasks = tokio::task::JoinSet::new();
    let (connection_shutdown, connection_shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::select! {
        _ = interrupt.as_mut() => {}
        _ = terminate.as_mut() => {}
        _ = state_root_removed.as_mut() => {}
        result = async {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let manager = runtime_manager.clone();
                        connection_tasks.spawn(conn_task_routed(
                            manager,
                            stream,
                            connection_shutdown_rx.clone(),
                        ));
                    }
                    Err(e) => append_log(&host_paths.log_path(), &format!("accept error: {}", e)),
                }
            }
        } => result,
    }

    let _ = scheduler_stop.try_send(());
    let _ = scheduler.await;
    connection_shutdown.send_replace(true);
    while connection_tasks.join_next().await.is_some() {}

    let _ = remove_listener_socket(&sock_path, &socket_metadata);
    if std::fs::symlink_metadata(&pid_path).is_ok_and(|current| same_inode(&pid_metadata, &current))
    {
        let _ = std::fs::remove_file(&pid_path);
    }
    Ok(())
}
