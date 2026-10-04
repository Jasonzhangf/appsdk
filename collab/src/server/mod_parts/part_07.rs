fn worker_identity_presence(server: &Server, worker: &WorkerRec) -> IdentityPresence {
    let Some(transport) = selected_transport_for_worker(worker) else {
        return IdentityPresence::Missing;
    };
    match transport.kind {
        TransportKind::Tmux => {
            let Some(endpoint) = transport.tmux_endpoint.as_ref() else {
                return IdentityPresence::Missing;
            };
            match crate::client::adapters::tmux::probe(endpoint) {
                Ok(crate::client::adapters::tmux::PanePresence::Present) => {
                    IdentityPresence::Present
                }
                Ok(crate::client::adapters::tmux::PanePresence::Missing) => {
                    IdentityPresence::Missing
                }
                Ok(crate::client::adapters::tmux::PanePresence::Unknown) | Err(_) => {
                    IdentityPresence::Unknown
                }
            }
        }
        TransportKind::AppServer => {
            let Some(thread_id) = transport.thread_id.as_deref() else {
                return IdentityPresence::Missing;
            };
            match (server.appserver_thread_status)(&transport, thread_id) {
                Ok(_) => IdentityPresence::Present,
                Err(error)
                    if error.contains("not found")
                        || error.contains("MISSING")
                        || error.contains("GONE") =>
                {
                    IdentityPresence::Missing
                }
                Err(_) => IdentityPresence::Unknown,
            }
        }
        TransportKind::Dsh => dsh_identity_presence(&transport),
    }
}

/// Presence of a dsh peer, judged only from what the gateway reports.
///
/// The gateway's agent status domain is exactly `running | inactive`, so a
/// reachable gateway answers with one of two states: running means Present, and
/// inactive means the agent is known but not live, which is Cold (resumable) and
/// emphatically not Missing. Missing is reserved for an explicit
/// `unknown-runtime` / `unknown-agent` denial; an unreachable, slow or
/// malformed gateway leaves the record Unknown so it is never retired.
fn dsh_identity_presence(transport: &SelectedTransport) -> IdentityPresence {
    let (Some(endpoint), Some(runtime_id), Some(agent_id)) = (
        transport.endpoint.as_deref(),
        transport.namespace.as_deref(),
        transport.thread_id.as_deref(),
    ) else {
        return IdentityPresence::Unknown;
    };
    match crate::client::adapters::dsh::facts(endpoint, runtime_id, agent_id) {
        Ok(facts) if facts.status == "running" => IdentityPresence::Present,
        Ok(_) => IdentityPresence::Cold,
        Err(error) if error.is_definitely_absent() => IdentityPresence::Missing,
        Err(_) => IdentityPresence::Unknown,
    }
}

fn merge_transport_presence(
    identity_presence: IdentityPresence,
    status_presence: IdentityPresence,
) -> IdentityPresence {
    match identity_presence {
        IdentityPresence::Present => status_presence,
        missing_or_unknown => missing_or_unknown,
    }
}

fn worker_presence_with_view(
    server: &Server,
    worker: &WorkerRec,
) -> (IdentityPresence, serde_json::Value) {
    if worker.transport.is_none() {
        return (
            worker_identity_presence(server, worker),
            serde_json::Value::Null,
        );
    }
    let identity_presence = worker_identity_presence(server, worker);
    let (status_presence, agent_view, _) = transport_agent_view(server, worker);
    let presence = merge_transport_presence(identity_presence, status_presence);
    let presence = match (
        presence,
        agent_view
            .get("thread_state")
            .and_then(serde_json::Value::as_str),
    ) {
        // A persisted thread that is cold on this endpoint still has a
        // verified identity and address, so it is Cold rather than Missing.
        // A system-error thread is genuinely unusable and stays Missing.
        (IdentityPresence::Present, Some("notLoaded")) => IdentityPresence::Cold,
        (IdentityPresence::Present, Some("systemError")) => IdentityPresence::Missing,
        _ => presence,
    };
    (presence, agent_view)
}

pub(crate) fn worker_presence(server: &Server, worker: &WorkerRec) -> IdentityPresence {
    worker_presence_with_view(server, worker).0
}

fn transport_agent_view(
    server: &Server,
    worker: &WorkerRec,
) -> (IdentityPresence, serde_json::Value, serde_json::Value) {
    let Some(transport) = selected_transport_for_worker(worker) else {
        return (
            IdentityPresence::Missing,
            serde_json::Value::Null,
            serde_json::Value::Null,
        );
    };
    if transport.kind == TransportKind::Tmux {
        let Some(endpoint) = transport.tmux_endpoint.as_ref() else {
            return (
                IdentityPresence::Missing,
                serde_json::json!({"transport": "tmux", "error": "TMUX_ENDPOINT_MISSING"}),
                serde_json::Value::Null,
            );
        };
        let presence = match crate::client::adapters::tmux::probe(endpoint) {
            Ok(crate::client::adapters::tmux::PanePresence::Present) => IdentityPresence::Present,
            Ok(crate::client::adapters::tmux::PanePresence::Missing) => IdentityPresence::Missing,
            Ok(crate::client::adapters::tmux::PanePresence::Unknown) | Err(_) => {
                return (
                    IdentityPresence::Unknown,
                    serde_json::json!({"transport": "tmux", "thread_state": "unknown"}),
                    serde_json::Value::Null,
                );
            }
        };
        return match crate::client::adapters::tmux::view(endpoint) {
            Ok(view) => (
                presence,
                serde_json::json!({
                    "thread_state": view.get("thread_state").cloned().unwrap_or(serde_json::Value::String("unknown".into())),
                    "active_flags": [],
                    "can_accept_direct_input": view.pointer("/thread/canAcceptDirectInput").cloned().unwrap_or(serde_json::Value::Bool(false)),
                    "latest_turn_status": view.pointer("/thread/turns/0/status").cloned().unwrap_or(serde_json::Value::Null),
                    "latest_turn_error": null,
                    "transport": "tmux",
                    "pane_output_changed": view.get("pane_output_changed").cloned().unwrap_or(serde_json::Value::Bool(false)),
                }),
                view,
            ),
            Err(error) => (
                IdentityPresence::Unknown,
                serde_json::json!({"transport": "tmux", "thread_state": "unknown", "error": error}),
                serde_json::Value::Null,
            ),
        };
    }
    if transport.kind == TransportKind::AppServer {
        let Some(thread_id) = transport.thread_id.as_deref() else {
            return (
                IdentityPresence::Missing,
                serde_json::json!({"transport": "appserver", "thread_state": "missing", "error": "APPSERVER_THREAD_ID_MISSING"}),
                serde_json::Value::Null,
            );
        };
        return match (server.appserver_thread_status)(&transport, thread_id) {
            Ok(raw) => {
                let thread_state = raw
                    .pointer("/thread/status/type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                // A response without `thread/status/type` is not evidence of a
                // resident thread. Reporting it as Present let the ledger mark
                // the binding Live on a malformed or truncated response and
                // suppressed repair; `identity.rs::classify_thread_status`
                // already treats the same shape as Unknown, so the two paths
                // disagreed.
                let identity_presence = match thread_state.as_deref() {
                    Some("notLoaded") => IdentityPresence::Cold,
                    Some("systemError") => IdentityPresence::Missing,
                    Some(_) => IdentityPresence::Present,
                    None => IdentityPresence::Unknown,
                };
                (
                    identity_presence,
                    serde_json::json!({
                        "thread_state": thread_state.as_deref().unwrap_or("unknown"),
                        "active_flags": raw.pointer("/activeTurns").cloned().unwrap_or(serde_json::json!([])),
                        "can_accept_direct_input": raw.pointer("/thread/canAcceptDirectInput").cloned().unwrap_or(serde_json::Value::Bool(false)),
                        "latest_turn_status": raw.pointer("/thread/latestTurnStatus").cloned().unwrap_or(serde_json::Value::Null),
                        "latest_turn_error": raw.pointer("/thread/latestTurnError").cloned().unwrap_or(serde_json::Value::Null),
                        "transport": "appserver",
                        "observed_at_ms": chrono::Utc::now().timestamp_millis(),
                    }),
                    raw,
                )
            }
            Err(error) => (
                IdentityPresence::Unknown,
                serde_json::json!({"transport": "appserver", "thread_state": "unknown", "error": error}),
                serde_json::Value::Null,
            ),
        };
    }
    if transport.kind == TransportKind::Dsh {
        let (Some(endpoint), Some(runtime_id), Some(agent_id)) = (
            transport.endpoint.as_deref(),
            transport.namespace.as_deref(),
            transport.thread_id.as_deref(),
        ) else {
            return (
                IdentityPresence::Unknown,
                serde_json::json!({"transport": "dsh", "thread_state": "unknown", "error": "DSH_ENDPOINT_INCOMPLETE"}),
                serde_json::Value::Null,
            );
        };
        return match crate::client::adapters::dsh::facts(endpoint, runtime_id, agent_id) {
            Ok(facts) => {
                // The gateway's status domain is `running | inactive`: running
                // is live, anything else is known-but-not-live, which is Cold.
                let running = facts.status == "running";
                let presence = if running {
                    IdentityPresence::Present
                } else {
                    IdentityPresence::Cold
                };
                let view = serde_json::json!({
                    "transport": "dsh",
                    "runtimeId": facts.runtime_id,
                    "agentId": facts.agent_id,
                    "sessionId": facts.session_id,
                    "cwd": facts.cwd,
                    "status": facts.status,
                });
                (
                    presence,
                    serde_json::json!({
                        "thread_state": facts.status,
                        "active_flags": [],
                        "can_accept_direct_input": running,
                        "latest_turn_status": serde_json::Value::Null,
                        "latest_turn_error": serde_json::Value::Null,
                        "transport": "dsh",
                        "observed_at_ms": chrono::Utc::now().timestamp_millis(),
                    }),
                    view,
                )
            }
            Err(error) if error.is_definitely_absent() => (
                IdentityPresence::Missing,
                serde_json::json!({"transport": "dsh", "thread_state": "missing", "error": error.to_string()}),
                serde_json::Value::Null,
            ),
            Err(error) => (
                IdentityPresence::Unknown,
                serde_json::json!({"transport": "dsh", "thread_state": "unknown", "error": error.to_string()}),
                serde_json::Value::Null,
            ),
        };
    }
    (
        IdentityPresence::Unknown,
        serde_json::json!({"transport": "unsupported", "thread_state": "unknown", "error": "TRANSPORT_UNSUPPORTED"}),
        serde_json::Value::Null,
    )
}

/// Pick a live registered peer without assigning work the peer already owns.
/// Tmux output can report idle as unknown before its observation window elapses;
/// the user-input conflict is outside this transport contract, so liveness is
/// the admission condition and pane state remains observational.
pub(crate) fn registered_available_peer_for_admission(
    server: &Server,
    requester: &str,
) -> Option<(String, String)> {
    // Snapshot only state-owned data while holding the mutex. Transport probes
    // run after the lock is released.
    let candidates: Vec<WorkerRec> = {
        let state = server.state.lock().unwrap();
        let mut workers: Vec<_> = state
            .workers
            .values()
            .filter(|worker| worker.id != requester)
            .filter(|worker| !is_managed_subagent(&state, &worker.id))
            .filter(|worker| {
                !state
                    .tasks
                    .values()
                    .any(|task| task.owner == worker.id && task_resource_active(&task.status))
            })
            .cloned()
            .collect();
        workers.sort_by(|a, b| a.id.cmp(&b.id));
        workers
    };

    let probed: Vec<WorkerRec> = candidates
        .into_iter()
        .filter(|worker| matches!(worker_presence(server, worker), IdentityPresence::Present))
        .collect();

    let state = server.state.lock().unwrap();
    for worker in probed {
        let Some(current) = state.workers.get(&worker.id) else {
            continue;
        };
        let unchanged = current.id == worker.id
            && current.token == worker.token
            && current.cwd == worker.cwd
            && current.registered_ms == worker.registered_ms;
        if unchanged
            && !is_managed_subagent(&state, &worker.id)
            && !state
                .tasks
                .values()
                .any(|task| task.owner == worker.id && task_resource_active(&task.status))
        {
            return Some((worker.id, "live registered peer has no active task".into()));
        }
    }
    None
}

pub(crate) fn idle_managed_subagent_for_admission(
    server: &Server,
    requester: &str,
) -> Option<(String, String, String)> {
    let candidates: Vec<(crate::subagent::Record, WorkerRec)> = {
        let state = server.state.lock().unwrap();
        let mut candidates = state
            .subagents
            .values()
            .filter(|record| record.parent == requester && record.status == "idle")
            .filter_map(|record| {
                state
                    .workers
                    .get(&record.peer)
                    .cloned()
                    .map(|worker| (record.clone(), worker))
            })
            .filter(|(_, worker)| {
                !state
                    .tasks
                    .values()
                    .any(|task| task.owner == worker.id && task_resource_active(&task.status))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.0.id.cmp(&right.0.id));
        candidates
    };

    let probed: Vec<(crate::subagent::Record, WorkerRec)> = candidates
        .into_iter()
        .filter(|(_, worker)| matches!(worker_presence(server, worker), IdentityPresence::Present))
        .collect();

    let state = server.state.lock().unwrap();
    for (record, worker) in probed {
        let Some(current) = state.subagents.get(&record.id) else {
            continue;
        };
        let Some(current_worker) = state.workers.get(&worker.id) else {
            continue;
        };
        if current.parent == requester
            && current.status == "idle"
            && current.peer == worker.id
            && current_worker.id == worker.id
            && current_worker.token == worker.token
            && current_worker.cwd == worker.cwd
            && current_worker.registered_ms == worker.registered_ms
            && !state
                .tasks
                .values()
                .any(|task| task.owner == worker.id && task_resource_active(&task.status))
        {
            return Some((
                record.id,
                worker.id,
                "live managed subagent is idle, owned, and has no active task".into(),
            ));
        }
    }
    None
}

pub(crate) fn live_master_id(
    server: &Server,
    state: &State,
) -> Result<Option<String>, &'static str> {
    let route_scope = server_route_scope(server, state)?;
    let Some(worker) = current_master_worker_id(state, route_scope.as_ref())
        .as_ref()
        .and_then(|id| state.workers.get(id))
    else {
        return Ok(None);
    };
    match worker_presence(server, worker) {
        IdentityPresence::Present => Ok(Some(worker.id.clone())),
        // Master authority requires a thread that is resident now: a cold
        // master cannot act on a request until something loads it, so it is
        // not treated as a live master.
        IdentityPresence::Cold => Ok(None),
        IdentityPresence::Missing => Ok(None),
        IdentityPresence::Unknown => Err(
            "master identity is unknown; defer authority changes until transport probes succeed",
        ),
    }
}

fn live_master_worker_snapshot(server: &Server) -> Result<Option<WorkerRec>, &'static str> {
    let worker = {
        let state = server.state.lock().unwrap();
        let route_scope = server_route_scope(server, &state)?;
        current_master_worker_id(&state, route_scope.as_ref())
            .as_ref()
            .and_then(|id| state.workers.get(id))
            .cloned()
    };
    let Some(worker) = worker else {
        return Ok(None);
    };
    match worker_presence(server, &worker) {
        IdentityPresence::Present => Ok(Some(worker)),
        IdentityPresence::Cold | IdentityPresence::Missing => Ok(None),
        IdentityPresence::Unknown => Err(
            "master identity is unknown; defer authority changes until transport probes succeed",
        ),
    }
}

fn is_managed_subagent(state: &State, worker_id: &str) -> bool {
    state
        .subagents
        .values()
        .any(|record| record.peer == worker_id)
}

fn ordinary_peer_presence_label(presence: IdentityPresence) -> Option<&'static str> {
    match presence {
        IdentityPresence::Present => Some("online"),
        IdentityPresence::Cold | IdentityPresence::Missing => Some("offline"),
        IdentityPresence::Unknown => None,
    }
}

pub(crate) fn live_managed_subagent_count(server: &Server, parent: &str) -> usize {
    let state = server.state.lock().unwrap();
    state
        .subagents
        .values()
        .filter(|record| record.parent == parent && record.status != "closed")
        .filter_map(|record| state.workers.get(&record.peer))
        .filter(|worker| matches!(worker_presence(server, worker), IdentityPresence::Present))
        .count()
}

include!("ordinary_presence_edges.rs");

fn record_scheduler_admission(server: &Server, admission: serde_json::Value) -> Result<(), Resp> {
    ensure_scheduler_admission_audit(server, &admission).map(|_| ())
}

#[cfg(test)]
fn scheduler_admit_subagent_start(
    _server: &Server,
    _worker_id: &str,
    _token: &str,
    _requested_id: Option<&str>,
    _requested_runtime: Option<&str>,
) -> Result<Option<Resp>, Resp> {
    Err(Resp::err(
        "MANAGED_SUBAGENT_UNSUPPORTED: start a peer in its own tmux pane",
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SchedulerAdmissionAuditState {
    Recorded,
    Failed(String),
}

fn scheduler_admission_audit_state(
    server: &Server,
    request_id: &str,
) -> Result<Option<SchedulerAdmissionAuditState>, Resp> {
    let path = server
        .storage_root
        .join(".agent-collab/server/events.jsonl");
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Resp::err(format!(
                "scheduler admission audit failed: lookup {error}"
            )))
        }
    };
    for line in content.lines() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if record.get("kind").and_then(serde_json::Value::as_str) != Some("scheduler_admission")
            || record
                .get("detail")
                .and_then(|detail| detail.get("request_id"))
                .and_then(serde_json::Value::as_str)
                != Some(request_id)
        {
            continue;
        }
        let detail = record.get("detail").cloned().unwrap_or_else(|| json!({}));
        if detail.get("status").and_then(serde_json::Value::as_str) == Some("failed") {
            let error = detail
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("scheduler admission audit failed")
                .to_string();
            return Ok(Some(SchedulerAdmissionAuditState::Failed(error)));
        }
        return Ok(Some(SchedulerAdmissionAuditState::Recorded));
    }
    Ok(None)
}

fn ensure_scheduler_admission_audit(
    server: &Server,
    admission: &serde_json::Value,
) -> Result<SchedulerAdmissionAuditState, Resp> {
    if let Some(request_id) = admission
        .get("request_id")
        .and_then(serde_json::Value::as_str)
    {
        if let Some(state) = scheduler_admission_audit_state(server, request_id)? {
            return Ok(state);
        }
    }
    if let Err(error) = record_activity(
        &server.storage_root,
        "scheduler_admission",
        admission.clone(),
    ) {
        append_log(
            &server.log_path(),
            &format!("SCHEDULER_ADMISSION_RECORD_FAILED: {error}"),
        );
        return Err(Resp::err(format!(
            "scheduler admission audit failed: {error}"
        )));
    }
    Ok(SchedulerAdmissionAuditState::Recorded)
}

fn scheduler_admission_audit_error(
    result: Result<SchedulerAdmissionAuditState, Resp>,
) -> Option<String> {
    match result {
        Ok(SchedulerAdmissionAuditState::Recorded) => None,
        Ok(SchedulerAdmissionAuditState::Failed(error)) => Some(error),
        Err(error) => Some(
            error
                .error
                .unwrap_or_else(|| "scheduler admission audit failed".into()),
        ),
    }
}

fn scheduler_admission_failed_response(
    admission: &crate::server::state::SchedulerAdmissionRecord,
) -> Resp {
    let error = admission.error.clone().unwrap_or_else(|| {
        "scheduler admission audit failed; retry with the same request_id".into()
    });
    Resp::err_data(
        error,
        json!({
            "request_id": admission.request_id,
            "reservation": true,
            "decision": "audit-failed",
            "message_id": admission.message_id,
            "task_id": admission.task_id,
            "admission": admission,
        }),
    )
}

fn scheduler_dispatch_recover_pending(server: &Server, request_id: &str) -> Option<Resp> {
    let pending = {
        let mut state = server.state.lock().unwrap();
        let admission = state.scheduler_admissions.get(request_id)?.clone();
        if admission.status == "notifying"
            && now_ms().saturating_sub(admission.updated_ms) >= state::REQUEST_COOLDOWN_MS
        {
            server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: request_id.into(),
                    status: "pending".into(),
                    error: admission.error.clone(),
                    updated_ms: now_ms(),
                }],
            );
        }
        state
            .scheduler_admissions
            .get(request_id)
            .filter(|admission| admission.status == "pending")
            .cloned()?
    };
    let (admission, task, subscription) = {
        let mut state = server.state.lock().unwrap();
        let Some(task) = state.tasks.get(&pending.task_id).cloned() else {
            return None;
        };
        let message_state = state
            .msgs
            .get(&pending.message_id)
            .map(|message| message.state.as_str())
            .unwrap_or("missing");
        // A native attempt that was already accepted cannot be resent. Its
        // stale notifying claim is reconciled to the accepted outcome instead
        // of opening a retry window; unknown outcomes keep their failure class
        // and are reported rather than silently resent.
        if state
            .notification_delivery_accepted
            .contains_key(&pending.message_id)
        {
            server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: request_id.into(),
                    status: "succeeded".into(),
                    error: None,
                    updated_ms: now_ms(),
                }],
            );
            let admission = state.scheduler_admissions.get(request_id).cloned()?;
            return Some(Resp::data(json!({
                "request_id": admission.request_id,
                "decision": admission.decision,
                "admission": {
                    "request_id": admission.request_id,
                    "decision": admission.decision,
                    "worker_id": admission.worker_id,
                    "managed_subagent_id": admission.managed_subagent_id,
                    "message_id": admission.message_id,
                    "task_id": admission.task_id,
                    "status": "succeeded",
                },
                "message_id": admission.message_id,
                "task_id": admission.task_id,
                "target": task.owner,
                "status": task.status,
                "managed_subagent_id": admission.managed_subagent_id,
                "managed_subagent": admission
                    .managed_subagent_id
                    .as_ref()
                    .map(|id| json!({"id": id, "worker_id": task.owner}))
                    .unwrap_or(serde_json::Value::Null),
                "notification": "already-accepted",
                "recovered": true,
            })));
        }
        if matches!(message_state, "read" | "delivered") {
            server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: request_id.into(),
                    status: "succeeded".into(),
                    error: None,
                    updated_ms: now_ms(),
                }],
            );
            let admission = state.scheduler_admissions.get(request_id).cloned()?;
            return Some(Resp::data(json!({
                "request_id": admission.request_id,
                "decision": admission.decision,
                "admission": {
                    "request_id": admission.request_id,
                    "decision": admission.decision,
                    "worker_id": admission.worker_id,
                    "managed_subagent_id": admission.managed_subagent_id,
                    "message_id": admission.message_id,
                    "task_id": admission.task_id,
                    "status": "succeeded",
                },
                "message_id": admission.message_id,
                "task_id": admission.task_id,
                "target": task.owner,
                "status": task.status,
                "managed_subagent_id": admission.managed_subagent_id,
                "managed_subagent": admission
                    .managed_subagent_id
                    .as_ref()
                    .map(|id| json!({"id": id, "worker_id": task.owner}))
                    .unwrap_or(serde_json::Value::Null),
                "notification": "already-consumed",
                "recovered": true,
            })));
        }
        let admission = state.scheduler_admissions.get(request_id).cloned()?;
        let subscription = state
            .matching_subscription(&admission.worker_id, "direct-message", None, now_ms())
            .cloned();
        (admission, task, subscription)
    };
    let notification_attempt = subscription.as_ref().map(|subscription| {
        attempt_scheduler_notification(server, request_id, &admission.message_id, &subscription.id)
    });
    if notification_attempt == Some(SchedulerNotificationAttempt::InFlight) {
        return Some(Resp::err_data(
            "scheduler dispatch notification is already in flight; retry with the same request_id",
            json!({
                "request_id": admission.request_id,
                "reservation": true,
                "decision": admission.decision,
                "message_id": admission.message_id,
                "task_id": admission.task_id,
            }),
        ));
    }
    let notified = notification_attempt == Some(SchedulerNotificationAttempt::Accepted);
    if subscription.is_some() && !notified {
        let error =
            "scheduler dispatch notification was not accepted by the selected App Server route";
        return Some(scheduler_notification_failed_response(&admission, error));
    }
    let mut data = json!({
        "request_id": admission.request_id,
        "decision": admission.decision,
        "admission": {
            "request_id": admission.request_id,
            "decision": admission.decision,
            "worker_id": admission.worker_id,
            "managed_subagent_id": admission.managed_subagent_id,
            "message_id": admission.message_id,
            "task_id": admission.task_id,
            "status": "succeeded",
        },
        "message_id": admission.message_id,
        "task_id": admission.task_id,
        "target": task.owner,
        "status": task.status,
        "managed_subagent_id": admission.managed_subagent_id,
        "managed_subagent": admission
            .managed_subagent_id
            .as_ref()
            .map(|id| json!({"id": id, "worker_id": task.owner}))
            .unwrap_or(serde_json::Value::Null),
        "notification": if subscription.is_none() {
            "mailbox-only-no-subscription"
        } else if notified {
            "sent"
        } else {
            "subscribed-not-sent"
        },
        "recovered": true,
    });
    if subscription.is_none() {
        apply_mailbox_only_repair_fields(&mut data);
    }
    Some(Resp::data(data))
}

fn scheduler_notification_failed_response(
    admission: &crate::server::state::SchedulerAdmissionRecord,
    error: &str,
) -> Resp {
    Resp::err_data(
        error,
        json!({
            "request_id": admission.request_id,
            "reservation": true,
            "decision": admission.decision,
            "message_id": admission.message_id,
            "task_id": admission.task_id,
            "admission": {
                "request_id": admission.request_id,
                "decision": admission.decision,
                "worker_id": admission.worker_id,
                "managed_subagent_id": admission.managed_subagent_id,
                "message_id": admission.message_id,
                "task_id": admission.task_id,
                "status": "pending",
                "error": error,
            },
        }),
    )
}

fn scheduler_dispatch_deduplicated(
    server: &Server,
    worker_id: &str,
    request_id: &str,
) -> Option<Resp> {
    let message_id = format!("scheduler-{request_id}");
    let task_id = format!("task-{message_id}");
    let state = server.state.lock().unwrap();
    if let Some(admission) = state.scheduler_admissions.get(request_id) {
        if admission.status == "failed" {
            return Some(scheduler_admission_failed_response(admission));
        }
        if admission.status == "pending" {
            return None;
        }
        if admission.status == "notifying" {
            return Some(Resp::err_data(
                "scheduler dispatch notification is already in flight; retry with the same request_id",
                json!({
                    "request_id": admission.request_id,
                    "reservation": true,
                    "decision": admission.decision,
                    "message_id": admission.message_id,
                    "task_id": admission.task_id,
                }),
            ));
        }
    }
    let (Some(message), Some(task)) = (
        state.msgs.get(&message_id).cloned(),
        state.tasks.get(&task_id).cloned(),
    ) else {
        return None;
    };
    let managed_subagent_id = state
        .subagents
        .values()
        .find(|record| record.parent == worker_id && record.peer == task.owner)
        .map(|record| record.id.clone());
    Some(Resp::data(json!({
        "request_id": request_id,
        "decision": "deduplicated",
        "message_id": message.id,
        "task_id": task.id,
        "target": task.owner,
        "status": task.status,
        "managed_subagent_id": managed_subagent_id,
        "managed_subagent": managed_subagent_id
            .as_ref()
            .map(|id| json!({"id": id, "worker_id": task.owner}))
            .unwrap_or(serde_json::Value::Null),
        "deduplicated": true,
    })))
}

fn scheduler_assignment_events(
    message: Message,
    task: TaskRec,
    managed_child: Option<crate::subagent::Record>,
) -> Vec<Event> {
    let message_id = message.id.clone();
    let mut events = vec![Event::Sent { msg: message }];
    events.push(Event::TaskCreated { task });
    if let Some(mut child) = managed_child {
        child.status = "assigned".into();
        child.last_message = Some(message_id);
        events.push(Event::SubagentUpdated { subagent: child });
    }
    events
}

pub(crate) fn handle_scheduler_dispatch(
    server: &Server,
    worker_id: String,
    token: String,
    request_id: String,
    subject: String,
    body: String,
    feature_id: Option<String>,
    mut worktree_path: Option<String>,
    branch: Option<String>,
    base_commit: Option<String>,
    priority: String,
    next_step: Option<String>,
) -> Resp {
    if !crate::subagent::valid_id(&request_id) {
        return Resp::err("scheduler request_id must be a valid non-empty ID");
    }
    if subject.trim().is_empty() || body.trim().is_empty() {
        return Resp::err("scheduler dispatch requires a non-empty subject and body");
    }
    if !matches!(priority.as_str(), "p0" | "p1" | "p2" | "p3" | "p4") {
        return Resp::err(format!(
            "invalid priority {}; must be p0, p1, p2, p3, or p4",
            priority
        ));
    }
    if let Some(path) = &worktree_path {
        let canonical = match validate_worktree_path(&server.root, &server.config, path) {
            Ok(path) => path,
            Err(error) => return Resp::err(error),
        };
        worktree_path = Some(canonical.display().to_string());
    }

    let authenticated = {
        let state = server.state.lock().unwrap();
        verify(&state, &worker_id, &token).is_ok()
    };
    if !authenticated {
        return Resp::err("scheduler dispatch authentication failed");
    }
    let master = {
        let state = server.state.lock().unwrap();
        let route_scope = match server_route_scope(server, &state) {
            Ok(route_scope) => route_scope,
            Err(error) => {
                return Resp::err(format!(
                    "scheduler dispatch requires a unique route scope: {error}"
                ))
            }
        };
        current_master_worker_id(&state, route_scope.as_ref())
            .as_ref()
            .and_then(|id| state.workers.get(id))
            .cloned()
    };
    let Some(master) = master else {
        return Resp::err("scheduler dispatch requires a live master");
    };
    if master.id != worker_id || worker_presence(server, &master) != IdentityPresence::Present {
        return Resp::err("scheduler dispatch requires the live registered master");
    }

    let message_id = format!("scheduler-{request_id}");
    let task_id = format!("task-{message_id}");
    for _ in 0..3 {
        if let Some(response) = scheduler_dispatch_recover_pending(server, &request_id) {
            return response;
        }
        if let Some(response) = scheduler_dispatch_deduplicated(server, &worker_id, &request_id) {
            return response;
        }
        let candidate = registered_available_peer_for_admission(server, &worker_id)
            .map(|(peer, reason)| (peer, None, reason, "use-registered-peer"))
            .or_else(|| {
                idle_managed_subagent_for_admission(server, &worker_id).map(
                    |(managed_id, peer, reason)| {
                        (
                            peer,
                            Some(managed_id),
                            reason,
                            "reuse-idle-managed-subagent",
                        )
                    },
                )
            });
        let Some((peer_id, managed_id, reason, decision)) = candidate else {
            if let Some(response) = scheduler_dispatch_recover_pending(server, &request_id) {
                return response;
            }
            if let Some(response) = scheduler_dispatch_deduplicated(server, &worker_id, &request_id)
            {
                return response;
            }
            return Resp::err(
                "MANAGED_SUBAGENT_UNSUPPORTED: no live registered tmux peer is available for dispatch",
            );
        };

        let mut state = server.state.lock().unwrap();
        let Some(worker) = state.workers.get(&peer_id).cloned() else {
            continue;
        };
        if state
            .tasks
            .values()
            .any(|task| task.owner == peer_id && task_resource_active(&task.status))
        {
            continue;
        }
        let mut managed_child = None;
        if let Some(managed_id) = &managed_id {
            let Some(child) = state.subagents.get(managed_id).cloned() else {
                continue;
            };
            if child.parent != worker_id || child.peer != peer_id || child.status != "idle" {
                continue;
            }
            managed_child = Some(child);
        } else if is_managed_subagent(&state, &peer_id) {
            continue;
        }
        if state.msgs.contains_key(&message_id) || state.tasks.contains_key(&task_id) {
            continue;
        }

        let now = now_ms();
        let message = Message {
            id: message_id.clone(),
            from: worker_id.clone(),
            to: peer_id.clone(),
            mtype: "notify".into(),
            subject: Some(subject.clone()),
            body: body.clone(),
            in_reply_to: None,
            created_ms: now,
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        };
        let task = TaskRec {
            id: task_id.clone(),
            owner: peer_id.clone(),
            created_by: worker_id.clone(),
            feature_id: feature_id.clone(),
            worktree_path: worktree_path.clone(),
            branch: branch.clone(),
            base_commit: base_commit.clone(),
            priority: priority.clone(),
            status: "assigned".into(),
            next_step: next_step.clone().or_else(|| {
                Some(format!(
                    "Read scheduler message {message_id}; mark task working before execution"
                ))
            }),
            wait: None,
            created_ms: now,
            updated_ms: now,
        };
        let admission_record = crate::server::state::SchedulerAdmissionRecord {
            request_id: request_id.clone(),
            decision: decision.into(),
            worker_id: peer_id.clone(),
            managed_subagent_id: managed_id.clone(),
            message_id: message_id.clone(),
            task_id: task_id.clone(),
            status: "pending".into(),
            error: None,
            created_ms: now,
            updated_ms: now,
        };
        let mut admission = json!({
            "request_id": request_id,
            "decision": decision,
            "worker_id": peer_id,
            "managed_subagent_id": managed_id,
            "message_id": message_id,
            "task_id": task_id,
            "reason": reason,
            "status": "pending",
        });
        let binding = worktree_binding_for_task(server, &task);
        let mut events = scheduler_assignment_events(message, task, managed_child);
        if let Some(binding) = binding {
            events.push(Event::WorktreeBound { binding });
        }
        let subscription = state
            .matching_subscription(&peer_id, "direct-message", None, now)
            .cloned();
        events.push(Event::DeliveryMode {
            msg_id: message_id.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        });
        if let Some(subscription) = &subscription {
            events.push(Event::WakeBound {
                message_id: message_id.clone(),
                subscription_id: subscription.id.clone(),
            });
        }
        events.push(Event::SchedulerAdmission {
            admission: admission_record,
        });
        server.commit_locked(&mut state, &events);
        if let Some(error) =
            scheduler_admission_audit_error(ensure_scheduler_admission_audit(server, &admission))
        {
            server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: request_id.clone(),
                    status: "failed".into(),
                    error: Some(error.clone()),
                    updated_ms: now_ms(),
                }],
            );
            drop(state);
            admission["status"] = json!("failed");
            admission["error"] = json!(error.clone());
            return Resp::err_data(
                error,
                json!({
                    "request_id": request_id,
                    "reservation": true,
                    "message_id": message_id,
                    "task_id": task_id,
                    "admission": admission,
                }),
            );
        }
        drop(state);
        let notification_attempt = subscription.as_ref().map(|subscription| {
            attempt_scheduler_notification(server, &request_id, &message_id, &subscription.id)
        });
        if notification_attempt == Some(SchedulerNotificationAttempt::InFlight) {
            return Resp::err_data(
                "scheduler dispatch notification is already in flight; retry with the same request_id",
                json!({
                    "request_id": request_id,
                    "reservation": true,
                    "decision": decision,
                    "message_id": message_id,
                    "task_id": task_id,
                }),
            );
        }
        let notified = notification_attempt == Some(SchedulerNotificationAttempt::Accepted);
        if subscription.is_some() && !notified {
            let admission_record = crate::server::state::SchedulerAdmissionRecord {
                request_id: request_id.clone(),
                decision: decision.into(),
                worker_id: peer_id.clone(),
                managed_subagent_id: managed_id.clone(),
                message_id: message_id.clone(),
                task_id: task_id.clone(),
                status: "pending".into(),
                error: None,
                created_ms: now,
                updated_ms: now,
            };
            return scheduler_notification_failed_response(
                &admission_record,
                "scheduler dispatch notification was not accepted by the selected App Server route",
            );
        }
        admission["status"] = json!("succeeded");
        let mut data = json!({
            "request_id": request_id,
            "decision": decision,
            "admission": admission,
            "message_id": message_id,
            "task_id": task_id,
            "target": peer_id,
            "status": "assigned",
            "managed_subagent_id": managed_id,
            "managed_subagent": managed_id
                .as_ref()
                .map(|id| json!({"id": id, "worker_id": peer_id}))
                .unwrap_or(serde_json::Value::Null),
            "notification": if subscription.is_none() {
                "mailbox-only-no-subscription"
            } else if notified {
                "sent"
            } else {
                "subscribed-not-sent"
            },
        });
        if subscription.is_none() {
            apply_mailbox_only_repair_fields(&mut data);
        }
        return Resp::data(data);
    }
    Resp::err(
        "scheduler dispatch capacity changed during admission; retry with the same request_id",
    )
}

fn verify_master_actor(
    server: &Server,
    state: &State,
    worker_id: &str,
    token: &str,
) -> Result<(), Resp> {
    let Some(worker) = state.workers.get(worker_id) else {
        return Err(Resp::err(format!("worker {} not registered", worker_id)));
    };
    if worker.token != token {
        return Err(Resp::err(
            "token mismatch: identity does not own this worker_id",
        ));
    }
    match live_master_id(server, state) {
        Ok(Some(master)) if master == worker_id => Ok(()),
        Ok(Some(_)) => Err(Resp::err(
            "master authority required; ask the registered master to delegate",
        )),
        Ok(None) => Err(Resp::err(
            "no live master; a peer may promote itself only with explicit user approval",
        )),
        Err(error) => Err(Resp::err(error)),
    }
}

/// Whether a distinct live worker has taken over the recorded master's pane.
///
/// A pane's `pane_pid` is the pane shell, so it survives a Codex restart inside
/// that pane: the pane probe behind `live_master_id` proves the *pane* is alive,
/// never that the recorded master still owns it. Only a same-scope worker that
/// is registered, whose *current* transport is still that pane, and which
/// probes `Present` counts as a takeover — the same route-plus-presence
/// invariant the close path enforces. A route left behind by a closed or
/// moved-away peer is not evidence, and neither is the absence of a route:
/// host-managed current-thread routes are published to the host server, so this
/// runtime's route index is not authoritative for ownership.
fn master_anchor_is_superseded(server: &Server, state: &State, master_worker_id: &str) -> bool {
    let Ok(route_scope) = server_route_scope(server, state) else {
        return false;
    };
    let Some(grant) = current_master_grant(state, route_scope.as_ref()) else {
        return false;
    };
    if grant.agent_id.as_str() != master_worker_id {
        return false;
    }
    let grant_scope = RouteScope {
        app_scope_id: grant.app_scope_id.clone(),
        project_scope_id: grant.project_scope.clone(),
    };
    let Some(endpoint) = state
        .global
        .lookup_binding_for(&grant_scope, &grant.binding_id)
        .and_then(|binding| binding.tmux_endpoint.as_ref())
        .cloned()
    else {
        // A transport without a pane anchor has no anchor for another
        // registration to take over.
        return false;
    };
    let owners = state
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .filter(|other| {
            other.route_scope() == grant_scope
                && other.binding_id != grant.binding_id
                && other.tmux_endpoint.as_ref().is_some_and(|other_endpoint| {
                    crate::client::adapters::tmux::same_pane_route(other_endpoint, &endpoint)
                })
        })
        .map(|other| other.agent_id.clone())
        .collect::<Vec<_>>();
    owners.into_iter().any(|agent_id| {
        state.workers.get(agent_id.as_str()).is_some_and(|worker| {
            // The taker's *current* transport must still be this pane: a worker
            // that moved away keeps its old binding and no longer owns it.
            let owns_pane = worker
                .transport
                .as_ref()
                .and_then(|transport| transport.tmux_endpoint.as_ref())
                .is_some_and(|current| {
                    crate::client::adapters::tmux::same_pane_route(current, &endpoint)
                });
            owns_pane && worker_presence(server, worker) == IdentityPresence::Present
        })
    })
}

fn handle_master_promote(
    server: &Server,
    worker_id: String,
    token: String,
    approval: String,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    let Some(worker) = state.workers.get(&worker_id).cloned() else {
        return Resp::err(format!("worker {} not registered", worker_id));
    };
    if worker.token != token {
        return Resp::err("token mismatch: identity does not own this worker_id");
    }
    if approval.trim().is_empty() {
        return Resp::err("master promotion requires explicit user approval");
    }
    match live_master_id(server, &state) {
        Ok(Some(master)) => {
            if !master_anchor_is_superseded(server, &state, &master) {
                return Resp::err("master already exists; only the registered master may delegate");
            }
            // The recorded master no longer owns its anchor. A pane's
            // `pane_pid` is the pane shell, so it survives a Codex restart in
            // that pane; `live_master_id` therefore still reports the recorded
            // master live. Letting that veto stand leaves the project with an
            // authority that can neither act on its anchor, nor be recovered,
            // nor be replaced. The explicit approval checked above is what
            // authorizes this repair.
        }
        Err(error) => return Resp::err(error),
        Ok(None) => {}
    }
    match worker_presence(server, &worker) {
        IdentityPresence::Present => {}
        // Promotion needs a master that can act immediately, so a cold thread
        // is refused with the same rule as a missing one.
        IdentityPresence::Cold => {
            return Resp::err("master promotion requires a live registered transport")
        }
        IdentityPresence::Missing => {
            return Resp::err("master promotion requires a live registered transport")
        }
        IdentityPresence::Unknown => return Resp::err(
            "promotion candidate identity is unknown; defer promotion until transport probes succeed",
        ),
    }
    let route_scope = match server_route_scope(server, &state) {
        Ok(Some(route_scope)) => route_scope,
        Ok(None) => return Resp::err("master promotion requires a registered project route"),
        Err(error) => return Resp::err(error),
    };
    let grant =
        match master_grant_for_worker(&state, &route_scope, &worker_id, &worker_id, &approval) {
            Ok(grant) => grant,
            Err(error) => return Resp::err(error),
        };
    let events = master_authority_transfer_events(&state, &route_scope, grant);
    server.commit_locked(&mut state, &events);
    Resp::data(json!({
        "master": worker_id,
        "mode": "user_approved_self_promotion",
        "role_brief": role_brief(server, &state, &worker_id)
    }))
}

fn handle_master_delegate(
    server: &Server,
    worker_id: String,
    token: String,
    target_id: String,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify_master_actor(server, &state, &worker_id, &token) {
        return error;
    }
    let Some(target) = state.workers.get(&target_id) else {
        return Resp::err(format!("target worker {} not registered", target_id));
    };
    match worker_presence(server, target) {
        IdentityPresence::Present => {}
        // Delegation goes through the same immediate notification path that
        // loads a cold thread, so a verified-but-cold target is acceptable.
        IdentityPresence::Cold => {}
        IdentityPresence::Missing => {
            return Resp::err("master delegation requires a live target transport")
        }
        IdentityPresence::Unknown => {
            return Resp::err(
                "delegation target identity is unknown; defer delegation until transport probes succeed",
            )
        }
    }
    let route_scope = match server_route_scope(server, &state) {
        Ok(Some(route_scope)) => route_scope,
        Ok(None) => return Resp::err("master delegation requires a registered project route"),
        Err(error) => return Resp::err(error),
    };
    let grant = match master_grant_for_worker(
        &state,
        &route_scope,
        &target_id,
        &worker_id,
        "delegated by the live master",
    ) {
        Ok(grant) => grant,
        Err(error) => return Resp::err(error),
    };
    let events = master_authority_transfer_events(&state, &route_scope, grant);
    server.commit_locked(&mut state, &events);
    Resp::data(json!({
        "master": target_id,
        "delegated_by": worker_id,
        "role_brief": role_brief(server, &state, &target_id)
    }))
}

fn handle_worker_close(
    server: &Server,
    worker_id: String,
    token: String,
    target_id: String,
    reason: String,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify_master_actor(server, &state, &worker_id, &token) {
        return error;
    }
    if reason.trim().is_empty() {
        return Resp::err("worker close requires a non-empty --reason");
    }
    if target_id == worker_id {
        return Resp::err("master cannot close itself; delegate first");
    }
    let Some(target) = state.workers.get(&target_id).cloned() else {
        let Some(closed) = state.worker_closures.get(&target_id).cloned() else {
            return Resp::err(format!("target worker {} not registered", target_id));
        };
        return Resp::data(json!({
            "closed": closed.worker_id,
            "closed_by": closed.closed_by,
            "reason": closed.reason,
            "snapshot_captured_ms": closed.snapshot_captured_ms,
            "at_ms": closed.at_ms,
            "reused": true,
        }));
    };
    // Closing a worker that still owns live work would strand the task and its
    // worktree. The task lifecycle must be resolved first.
    let owned: Vec<String> = state
        .tasks
        .values()
        .filter(|task| task.owner == target_id && keepalive::unfinished(&task.status))
        .map(|task| task.id.clone())
        .collect();
    if !owned.is_empty() {
        return Resp::err(format!(
            "worker {} still owns {}; close or force-close the task first",
            target_id,
            owned.join(", ")
        ));
    }

    let Some(target_thread_id) = target
        .transport
        .as_ref()
        .and_then(|transport| transport.thread_id.as_deref())
    else {
        return Resp::err(format!(
            "worker {} has no bound App Server thread; snapshot evidence is unavailable",
            target_id
        ));
    };
    let snapshot = state
        .worker_snapshots
        .get(&target_id)
        .filter(|receipt| receipt.thread_id == target_thread_id)
        .map(|receipt| receipt.captured_ms);
    let Some(snapshot_captured_ms) = snapshot else {
        return Resp::err(format!(
            "worker {} requires a successful worker snapshot for its bound App Server thread before close",
            target_id
        ));
    };

    let now = now_ms();
    server.commit_locked(
        &mut state,
        &[Event::WorkerClosed {
            worker_id: target_id.clone(),
            closed_by: worker_id.clone(),
            reason: reason.clone(),
            snapshot_captured_ms: Some(snapshot_captured_ms),
            at_ms: now,
        }],
    );
    Resp::data(json!({
        "closed": target_id,
        "closed_by": worker_id,
        "reason": reason,
        "snapshot_captured_ms": snapshot_captured_ms,
        "transport": target.transport,
    }))
}

fn handle_worker_snapshot(
    server: &Server,
    worker_id: String,
    token: String,
    target_id: String,
    lines: usize,
) -> Resp {
    let state = server.state.lock().unwrap();
    if let Err(error) = verify_master_actor(server, &state, &worker_id, &token) {
        return error;
    }
    let Some(target) = state.workers.get(&target_id) else {
        return Resp::err(format!("target worker {} not registered", target_id));
    };
    if target.transport.is_none() {
        return Resp::err(format!("worker {} has no registered transport", target_id));
    }
    if !(1..=200).contains(&lines) {
        return Resp::err("snapshot lines must be 1..200");
    }
    Resp::err("WORKER_SNAPSHOT_UNSUPPORTED: tmux panes do not expose durable Codex thread history; inspect the worker's durable mailbox and task state")
}
