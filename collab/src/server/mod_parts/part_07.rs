// Project binding truth is independent of the host's current-route index.
fn current_worker_binding<'a>(
    state: &'a State,
    project_scope: &crate::scope::ProjectScopeId,
    app_scope: Option<&AppServerId>,
    worker: &WorkerRec,
) -> Option<&'a RuntimeBinding> {
    let transport = worker.transport.as_ref()?;
    let project = state.global.lookup_project(project_scope)?;
    let mut bindings = project.runtime_bindings.values().filter(|binding| {
        binding.agent_id.as_str() == worker.id
            && app_scope.is_none_or(|app| binding.app_scope_id == *app)
            && binding.tmux_endpoint == transport.tmux_endpoint
            && binding.session_id.as_ref().map(|id| id.as_str()) == transport.session_id.as_deref()
            && binding.native_thread_id.as_ref().map(|id| id.as_str())
                == transport.thread_id.as_deref()
    });
    let binding = bindings.next()?;
    bindings.next().is_none().then_some(binding)
}

// Pure persisted identity proof. Callers holding the reducer lock must use
// their current snapshot instead of acquiring server.state again.
fn tmux_identity_presence(server: &Server, state: &State, worker: &WorkerRec) -> IdentityPresence {
    let Some(endpoint) = worker
        .transport
        .as_ref()
        .and_then(|transport| transport.tmux_endpoint.as_ref())
    else {
        return IdentityPresence::Missing;
    };
    let current = state.workers.get(&worker.id).is_some_and(|current| {
        current.token == worker.token && current.transport == worker.transport
    });
    let Ok(project_scope) = GlobalState::canonical_project_scope(&server.root) else {
        return IdentityPresence::Missing;
    };
    let owned = current_worker_binding(state, &project_scope, None, worker)
        .is_some_and(|binding| binding.tmux_endpoint.as_ref() == Some(endpoint));
    if current && owned {
        IdentityPresence::Present
    } else {
        IdentityPresence::Missing
    }
}

fn desktop_identity_presence(server: &Server, state: &State, worker: &WorkerRec) -> IdentityPresence {
    let Ok(project) = GlobalState::canonical_project_scope(&server.root) else {
        return IdentityPresence::Missing;
    };
    if state.workers.get(&worker.id).is_some_and(|current| {
        current.token == worker.token && current.transport == worker.transport
    })
        && current_worker_binding(state, &project, None, worker).is_some()
    {
        IdentityPresence::Present
    } else {
        IdentityPresence::Missing
    }
}

fn rex_identity_presence(_server: &Server, state: &State, worker: &WorkerRec) -> IdentityPresence {
    let Some(transport) = worker.transport.as_ref() else {
        return IdentityPresence::Missing;
    };
    let has_endpoint = transport.rex_endpoint.is_some();
    let has_session = transport
        .session_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    if !has_endpoint || !has_session {
        return IdentityPresence::Missing;
    }
    let current = state.workers.get(&worker.id).is_some_and(|current| {
        current.token == worker.token && current.transport == worker.transport
    });
    if current {
        IdentityPresence::Present
    } else {
        IdentityPresence::Missing
    }
}

fn worker_identity_presence(server: &Server, worker: &WorkerRec) -> IdentityPresence {
    let Some(transport) = selected_transport_for_worker(worker) else {
        return IdentityPresence::Missing;
    };
    match transport.kind {
        TransportKind::DesktopBridge => {
            let state = server.state.lock().unwrap();
            desktop_identity_presence(server, &state, worker)
        }
        TransportKind::Tmux => {
            let state = server.state.lock().unwrap();
            tmux_identity_presence(server, &state, worker)
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
        TransportKind::Rex => {
            let state = server.state.lock().unwrap();
            rex_identity_presence(server, &state, worker)
        }
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

/// Read periodic facts only for the current registered agent session.
fn worker_agent_facts<'a>(
    server: &Server,
    state: &'a State,
    worker: &WorkerRec,
) -> (
    Option<&'a name_state::AgentPresenceRecord>,
    Option<&'a name_state::AgentActivityRecord>,
) {
    if state.workers.get(&worker.id) != Some(worker) {
        return (None, None);
    }
    let Some(transport) = worker.transport.as_ref() else {
        return (None, None);
    };
    let session = transport
        .session_id
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            transport
                .tmux_endpoint
                .as_ref()?
                .codex_session_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
        });
    let Some(session) = session else {
        return (None, None);
    };
    let Ok(scope) = GlobalState::canonical_project_scope(&server.root) else {
        return (None, None);
    };
    (
        state
            .agent_presence(scope.as_str(), &worker.id)
            .filter(|fact| fact.session_id == session),
        state
            .agent_activity(scope.as_str(), &worker.id)
            .filter(|fact| fact.session_id == session),
    )
}

fn worker_agent_view(
    server: &Server,
    worker: &WorkerRec,
) -> Option<(IdentityPresence, serde_json::Value)> {
    let state = server.state.lock().unwrap();
    worker_agent_view_in_state(server, &state, worker)
}

fn worker_agent_view_in_state(
    server: &Server,
    state: &State,
    worker: &WorkerRec,
) -> Option<(IdentityPresence, serde_json::Value)> {
    let (presence, activity) = worker_agent_facts(server, state, worker);
    if presence.is_none() && activity.is_none() {
        return None;
    }
    let observed_presence = match presence.map(|fact| fact.presence) {
        Some(name_state::AgentPresence::Online) => IdentityPresence::Present,
        Some(name_state::AgentPresence::Offline) => IdentityPresence::Missing,
        _ => IdentityPresence::Unknown,
    };
    Some((
        observed_presence,
        serde_json::json!({
            "agent_presence": presence.map(|fact| fact.presence),
            "activity": activity.map(|fact| fact.activity),
            "background": activity.map(|fact| fact.background),
            "presence_observation": presence,
            "activity_observation": activity,
            "thread_state": match activity.map(|fact| fact.activity) {
                Some(name_state::AgentActivity::Busy) => "working",
                Some(name_state::AgentActivity::Idle) => "idle",
                Some(name_state::AgentActivity::WaitingUser) => "waiting_user",
                Some(name_state::AgentActivity::Error) => "error",
                _ => "unknown",
            },
        }),
    ))
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
    if let Some(observation) = worker_agent_view(server, worker) {
        return observation;
    }
    worker_transport_presence_with_view(server, worker)
}

fn worker_transport_presence_with_view(
    server: &Server,
    worker: &WorkerRec,
) -> (IdentityPresence, serde_json::Value) {
    let identity_presence = worker_identity_presence(server, worker);
    let (status_presence, agent_view, _) = transport_agent_view(server, worker);
    if selected_transport_for_worker(worker)
        .is_some_and(|transport| transport.kind == TransportKind::Tmux)
    {
        // Capture/view failure is activity telemetry. The persisted pane
        // binding remains the identity and address decision.
        return (identity_presence, agent_view);
    }
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

fn worker_presence_in_state(
    server: &Server,
    state: &State,
    worker: &WorkerRec,
) -> IdentityPresence {
    if selected_transport_for_worker(worker)
        .is_some_and(|transport| transport.kind == TransportKind::DesktopBridge)
    {
        let identity = desktop_identity_presence(server, state, worker);
        let status = worker_agent_view_in_state(server, state, worker)
            .map(|view| view.0)
            .unwrap_or_else(|| transport_agent_view(server, worker).0);
        return merge_transport_presence(identity, status);
    }
    if selected_transport_for_worker(worker)
        .is_some_and(|transport| transport.kind == TransportKind::Tmux)
    {
        tmux_identity_presence(server, state, worker)
    } else if selected_transport_for_worker(worker)
        .is_some_and(|transport| transport.kind == TransportKind::Rex)
    {
        // Rex identity is proved from the held state, but the Rex monitor keeps
        // agent presence independent of program status. Combine the identity
        // check with the recorded observation (Unknown by default) without
        // re-entering the state lock; falling through to
        // worker_transport_presence_with_view would deadlock the holder.
        let identity = rex_identity_presence(server, state, worker);
        let observed = worker_agent_view_in_state(server, state, worker)
            .map(|view| view.0)
            .unwrap_or(IdentityPresence::Unknown);
        merge_transport_presence(identity, observed)
    } else {
        worker_agent_view_in_state(server, state, worker)
            .unwrap_or_else(|| worker_transport_presence_with_view(server, worker))
            .0
    }
}

pub(crate) fn worker_presence(server: &Server, worker: &WorkerRec) -> IdentityPresence {
    if selected_transport_for_worker(worker)
        .is_some_and(|transport| transport.kind == TransportKind::Tmux)
    {
        return worker_identity_presence(server, worker);
    }
    worker_presence_with_view(server, worker).0
}

/// Presence for the automatic stale-identity maintenance decision.
///
/// Only the transport adapter's own result may grant definitive absence.
/// Persisted agent presence/activity telemetry (including a stale `Offline`
/// observation) is never consulted here, so it cannot retire an App Server
/// thread that the adapter still reports as resident.
pub(crate) fn maintenance_presence(
    server: &Server,
    target: &CloseEndpointTarget,
) -> IdentityPresence {
    let Some(worker) = target.worker.as_ref() else {
        return IdentityPresence::Missing;
    };
    let Some(transport) = selected_transport_for_worker(worker) else {
        return IdentityPresence::Unknown;
    };
    match transport.kind {
        TransportKind::Tmux => close_endpoint_presence(server, target),
        TransportKind::AppServer => {
            let Some(thread_id) = transport.thread_id.as_deref() else {
                return IdentityPresence::Unknown;
            };
            thread_status_presence((server.appserver_thread_status)(&transport, thread_id))
        }
        TransportKind::DesktopBridge => {
            let Some(thread_id) = transport.thread_id.as_deref() else {
                return IdentityPresence::Unknown;
            };
            // Maintenance presence must come from the DesktopBridge adapter's
            // current thread fact, not from a persisted local projection. The
            // default oracle routes DesktopBridge to read_thread_status.
            thread_status_presence((server.appserver_thread_status)(&transport, thread_id))
        }
        TransportKind::Dsh => dsh_identity_presence(&transport),
        TransportKind::Rex => rex_maintenance_presence(&transport),
    }
}

/// Rex maintenance presence is decided by the adapter's exact-block probe.
/// Only a definitive exact-target absence may retire the execution binding;
/// generation, malformed and unknown failures stay Unknown so the persistent
/// identity and business duties are preserved.
fn rex_maintenance_presence(transport: &SelectedTransport) -> IdentityPresence {
    let Some(endpoint) = transport.rex_endpoint.as_ref() else {
        return IdentityPresence::Missing;
    };
    if endpoint.session_id.trim().is_empty() {
        return IdentityPresence::Missing;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return IdentityPresence::Unknown,
    };
    match runtime.block_on(crate::client::adapters::rex::resolve_target(endpoint)) {
        Ok(()) => IdentityPresence::Present,
        Err(error) if crate::client::adapters::rex::resolve_error_is_definitively_absent(&error) => {
            IdentityPresence::Missing
        }
        Err(_) => IdentityPresence::Unknown,
    }
}

// Pin only the observed owner and its bindings, not the reducer revision:
// unrelated timer writes do not invalidate an endpoint observation.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CloseEndpointTarget {
    worker: Option<WorkerRec>,
    bindings: Vec<RuntimeBinding>,
    routes: Vec<RuntimeBinding>,
    legacy_routes: Vec<RuntimeBinding>,
}

pub(crate) fn close_endpoint_target(state: &State, worker_id: &str) -> CloseEndpointTarget {
    CloseEndpointTarget {
        worker: state.workers.get(worker_id).cloned(),
        bindings: state
            .global
            .projects
            .values()
            .flat_map(|project| {
                project
                    .runtime_bindings
                    .values()
                    .filter(|binding| binding.agent_id.as_str() == worker_id)
                    .cloned()
            })
            .collect(),
        routes: state
            .global
            .current_thread_routes
            .values()
            .filter(|binding| binding.agent_id.as_str() == worker_id)
            .cloned()
            .collect(),
        legacy_routes: state
            .global
            .legacy_thread_routes
            .values()
            .filter(|binding| binding.agent_id.as_str() == worker_id)
            .cloned()
            .collect(),
    }
}

/// Classify one adapter-owned thread/status fact. Only an explicit statement
/// that the exact thread is absent is definitive Missing; notSubmitted, target
/// mismatch, system errors and other transport failures stay Unknown.
fn thread_status_presence(result: Result<serde_json::Value, String>) -> IdentityPresence {
    match result {
        Ok(raw) => match raw
            .pointer("/thread/status/type")
            .and_then(serde_json::Value::as_str)
        {
            Some("notLoaded") => IdentityPresence::Cold,
            // A resident thread in an error state is not definitively absent, so
            // retain it instead of retiring its control plane.
            Some("systemError") => IdentityPresence::Unknown,
            Some(_) => IdentityPresence::Present,
            None => IdentityPresence::Unknown,
        },
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

/// Observe the selected address outside the reducer lock. Tmux identity is
/// durable; only the existing probe's definitive Missing grants an exemption.
pub(crate) fn close_endpoint_presence(
    server: &Server,
    target: &CloseEndpointTarget,
) -> IdentityPresence {
    let Some(worker) = target.worker.as_ref() else {
        return IdentityPresence::Missing;
    };
    let Some(transport) = selected_transport_for_worker(worker) else {
        return IdentityPresence::Unknown;
    };
    if transport.kind != TransportKind::Tmux {
        return worker_presence(server, worker);
    }
    let Some(endpoint) = transport.tmux_endpoint.as_ref() else {
        return IdentityPresence::Unknown;
    };
    let mut bindings = target
        .bindings
        .iter()
        .filter(|binding| binding.tmux_endpoint.as_ref() == Some(endpoint));
    let Some(binding) = bindings.next() else {
        return IdentityPresence::Unknown;
    };
    if bindings.next().is_some() || !target.routes.contains(binding) {
        return IdentityPresence::Unknown;
    }
    match crate::client::adapters::tmux::probe(endpoint) {
        Ok(crate::client::adapters::tmux::PanePresence::Missing) => IdentityPresence::Missing,
        Ok(crate::client::adapters::tmux::PanePresence::Present) => IdentityPresence::Present,
        _ => IdentityPresence::Unknown,
    }
}

#[cfg(test)]
mod close_endpoint_fence_tests {
    use super::*;

    #[test]
    fn owner_fence_allows_unrelated_writes_but_rejects_current_token_transport_and_binding_changes() {
        let mut state = State::default();
        state.workers.insert(
            "peer".into(),
            WorkerRec {
                id: "peer".into(),
                token: "token".into(),
                cwd: "/project".into(),
                registered_ms: 1,
                transport: None,
            },
        );
        let binding = RuntimeBinding::new(
            ProjectScopeId::new("/project").unwrap(),
            AppServerId::new("app").unwrap(),
            AgentId::new("peer").unwrap(),
            RuntimeId::new("runtime").unwrap(),
            BindingId::new("binding-peer").unwrap(),
            None,
        )
        .unwrap();
        state
            .global
            .current_thread_routes
            .insert(("session".into(), "thread".into()), binding);
        let observed = close_endpoint_target(&state, "peer");
        state.global.revision += 1;
        state
            .keepalives
            .insert("other".into(), keepalive::Record::default());
        assert!(close_endpoint_target(&state, "peer") == observed);
        state.workers.get_mut("peer").unwrap().token = "replacement".into();
        assert!(close_endpoint_target(&state, "peer") != observed);
        state.workers.get_mut("peer").unwrap().token = "token".into();
        state.workers.get_mut("peer").unwrap().transport = Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///new".into()),
            namespace: Some("codex_app".into()),
            session_id: Some("session".into()),
            thread_id: Some("thread".into()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec![],
            self_check: "bound".into(),
        });
        assert!(close_endpoint_target(&state, "peer") != observed);
        state.workers.get_mut("peer").unwrap().transport = None;
        state
            .global
            .current_thread_routes
            .values_mut()
            .next()
            .unwrap()
            .runtime_id = RuntimeId::new("runtime-replacement").unwrap();
        assert!(close_endpoint_target(&state, "peer") != observed);
    }
}

/// Project identity validity separately from activity/address reachability.
///
/// A persisted transport address proves the registered identity even when the
/// current runtime state is unknown. `None` reachability means the probe could
/// not decide; it is not a negative identity result.
pub(crate) fn selected_transport_endpoint_live(
    server: &Server,
    transport: &SelectedTransport,
) -> Option<bool> {
    match transport.kind {
        TransportKind::DesktopBridge => transport
            .thread_id
            .as_deref()
            .and_then(|thread| (server.appserver_thread_status)(transport, thread).ok())
            .map(|_| true),
        TransportKind::Tmux => match transport
            .tmux_endpoint
            .as_ref()
            .map(crate::client::adapters::tmux::probe)
        {
            Some(Ok(crate::client::adapters::tmux::PanePresence::Present)) => Some(true),
            Some(Ok(crate::client::adapters::tmux::PanePresence::Missing)) => Some(false),
            Some(Ok(crate::client::adapters::tmux::PanePresence::Unknown))
            | Some(Err(_))
            | None => None,
        },
        TransportKind::AppServer => transport
            .thread_id
            .as_deref()
            .and_then(|thread| match (server.appserver_thread_status)(transport, thread) {
                Ok(_) => Some(true),
                Err(error)
                    if error.contains("not found")
                        || error.contains("MISSING")
                        || error.contains("GONE") =>
                {
                    Some(false)
                }
                Err(_) => None,
            }),
        TransportKind::Dsh => {
            let (Some(endpoint), Some(runtime_id), Some(agent_id)) = (
                transport.endpoint.as_deref(),
                transport.namespace.as_deref(),
                transport.thread_id.as_deref(),
            ) else {
                return None;
            };
            match crate::client::adapters::dsh::facts(endpoint, runtime_id, agent_id) {
                Ok(_) => Some(true),
                Err(error) if error.is_definitely_absent() => Some(false),
                Err(_) => None,
            }
        }
        TransportKind::Rex => None,
    }
}

pub(crate) fn worker_identity_and_endpoint(
    server: &Server,
    worker: &WorkerRec,
) -> (bool, Option<bool>) {
    let Some(transport) = selected_transport_for_worker(worker) else {
        return (false, Some(false));
    };
    match transport.kind {
        TransportKind::DesktopBridge => {
            let valid = worker_identity_presence(server, worker) == IdentityPresence::Present;
            let live = transport.thread_id.as_deref().and_then(|thread| {
                (server.appserver_thread_status)(&transport, thread).ok().map(|_| true)
            });
            (valid, live)
        }
        TransportKind::Tmux => {
            let identity_valid =
                worker_identity_presence(server, worker) == IdentityPresence::Present;
            let endpoint_live = match transport
                .tmux_endpoint
                .as_ref()
                .map(crate::client::adapters::tmux::probe)
            {
                Some(Ok(crate::client::adapters::tmux::PanePresence::Present)) => Some(true),
                Some(Ok(crate::client::adapters::tmux::PanePresence::Missing)) => Some(false),
                Some(Ok(crate::client::adapters::tmux::PanePresence::Unknown))
                | Some(Err(_))
                | None => None,
            };
            (identity_valid, endpoint_live)
        }
        TransportKind::AppServer | TransportKind::Dsh => {
            let identity_valid = if transport.kind == TransportKind::AppServer {
                transport.thread_id.is_some()
            } else {
                transport.endpoint.is_some()
                    && transport.namespace.is_some()
                    && transport.thread_id.is_some()
            };
            let endpoint_live = match worker_identity_presence(server, worker) {
                IdentityPresence::Present | IdentityPresence::Cold => Some(true),
                IdentityPresence::Missing => Some(false),
                IdentityPresence::Unknown => None,
            };
            (identity_valid, endpoint_live)
        }
        TransportKind::Rex => {
            let identity_valid = transport.rex_endpoint.is_some()
                && transport
                    .session_id
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty());
            (identity_valid, None)
        }
    }
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
    if matches!(transport.kind, TransportKind::AppServer | TransportKind::DesktopBridge) {
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
                        "latest_turn_status": raw.pointer("/thread/latestTurnStatus").or_else(|| raw.pointer("/turns/0/status")).cloned().unwrap_or(serde_json::Value::Null),
                        "latest_turn_error": raw.pointer("/thread/latestTurnError").cloned().unwrap_or(serde_json::Value::Null),
                        "transport": transport.kind.as_str(),
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
    if transport.kind == TransportKind::Rex {
        return match transport.rex_endpoint.as_ref() {
            None => (
                IdentityPresence::Missing,
                serde_json::json!({"transport": "rex", "error": "REX_ENDPOINT_MISSING"}),
                serde_json::Value::Null,
            ),
            Some(_) => match worker_agent_view(server, worker) {
                Some((presence, view)) => (presence, view, serde_json::Value::Null),
                None => (
                    IdentityPresence::Unknown,
                    serde_json::json!({"transport": "rex", "thread_state": "unknown"}),
                    serde_json::Value::Null,
                ),
            },
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

/// Pick an identity-valid registered peer without assigning work it already owns.
/// Endpoint activity does not gate durable task admission; the notification
/// owner separately records submission and wake outcomes.
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
            .filter(|worker| !board_peer_has_execution_reservation(&state, &worker.id, None))
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
            && !board_peer_has_execution_reservation(&state, &worker.id, None)
        {
            return Some((
                worker.id,
                "identity-valid registered peer has no active task".into(),
            ));
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
            .filter(|(_, worker)| !board_peer_has_execution_reservation(&state, &worker.id, None))
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
            && !board_peer_has_execution_reservation(&state, &worker.id, None)
        {
            return Some((
                record.id,
                worker.id,
                "identity-valid managed subagent is idle, owned, and has no active task".into(),
            ));
        }
    }
    None
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
        .filter(|worker| {
            matches!(
                worker_presence_in_state(server, &state, worker),
                IdentityPresence::Present
            )
        })
        .count()
}

include!("ordinary_presence_edges.rs");

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
            if let Err(error) = server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: request_id.into(),
                    status: "pending".into(),
                    error: admission.error.clone(),
                    updated_ms: now_ms(),
                }],
            ) {
                return Some(Resp::err(format!("SCHEDULER_DURABILITY_FAILED: {error}")));
            }
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
            if let Err(error) = server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: request_id.into(),
                    status: "succeeded".into(),
                    error: None,
                    updated_ms: now_ms(),
                }],
            ) {
                return Some(Resp::err(format!("SCHEDULER_DURABILITY_FAILED: {error}")));
            }
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
            if let Err(error) = server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: request_id.into(),
                    status: "succeeded".into(),
                    error: None,
                    updated_ms: now_ms(),
                }],
            ) {
                return Some(Resp::err(format!("SCHEDULER_DURABILITY_FAILED: {error}")));
            }
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
    let notification_attempt = match subscription.as_ref().map(|subscription| {
        attempt_scheduler_notification(server, request_id, &admission.message_id, &subscription.id)
    }).transpose() { Ok(outcome) => outcome, Err(error) => return Some(error) };
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
    let consumed = notification_attempt == Some(SchedulerNotificationAttempt::Consumed);
    if subscription.is_some() && !notified && !consumed {
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
        } else if consumed {
            "already-consumed"
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

/// Authorize a scheduler-dispatch caller against the current typed grant for
/// this route. The token and the resolved project/app scope are the existing
/// admission, and transport liveness is deliberately not consulted.
fn scheduler_dispatch_authorized(
    server: &Server,
    state: &State,
    worker_id: &str,
    token: &str,
) -> Result<(), Resp> {
    verify(state, worker_id, token)
        .map_err(|_| Resp::err("scheduler dispatch authentication failed"))?;
    let route_scope = server_route_scope(server, state).map_err(|error| {
        Resp::err(format!(
            "scheduler dispatch requires a unique route scope: {error}"
        ))
    })?;
    if current_master_worker_id(state, route_scope.as_ref()).as_deref() != Some(worker_id) {
        return Err(Resp::err(
            "scheduler dispatch requires the current registered master authority",
        ));
    }
    Ok(())
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
        match declare_task_workspace(&server.root, &server.config, path) {
            Ok(declared) => worktree_path = Some(declared),
            Err(error) => return Resp::err(error),
        }
    }

    let message_id = format!("scheduler-{request_id}");
    let task_id = format!("task-{message_id}");
    for _ in 0..3 {
        // Authorization is the current typed grant, not a transport probe: a
        // tmux pane address can never answer whether the holder is live, so the
        // recorded holder keeps control while its agent runtime stays Unknown.
        // A retry or recovery re-enters here after releasing the lock, so the
        // grant is re-read on every attempt.
        {
            let state = server.state.lock().unwrap();
            if let Err(error) = scheduler_dispatch_authorized(server, &state, &worker_id, &token) {
                return error;
            }
        }
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

        if managed_id.is_none() {
            return Resp::err_data(
                "BOARD_INVITATION_REQUIRED: ordinary peers must receive a board invitation with delivery/test conditions and an observed revision; use collab board publish/invite",
                json!({"peer_id": peer_id, "assigned": false, "next_action": "collab board publish then collab board invite"}),
            );
        }
        let mut state = server.state.lock().unwrap();
        // Candidate probing above released the lock, so re-check the current
        // typed grant before reserving capacity. This is the existing
        // concurrency boundary, not a new ledger: a caller whose authority
        // changed during admission must not commit the reservation.
        if let Err(error) = scheduler_dispatch_authorized(server, &state, &worker_id, &token) {
            return error;
        }
        if !state.workers.contains_key(&peer_id) {
            continue;
        }
        if board_peer_has_execution_reservation(&state, &peer_id, None) {
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
            isolation_mode: worktree_path
                .as_deref()
                .map(|path| derive_isolation_mode(Some(path))),
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
        if let Err(error) = server.commit_locked(&mut state, &events) {
            return Resp::err(format!("SCHEDULER_DURABILITY_FAILED: {error}"));
        }
        drop(state);
        let notification_attempt = match subscription.as_ref().map(|subscription| {
            attempt_scheduler_notification(server, &request_id, &message_id, &subscription.id)
        }).transpose() { Ok(outcome) => outcome, Err(error) => return error };
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
        let consumed = notification_attempt == Some(SchedulerNotificationAttempt::Consumed);
        if subscription.is_some() && !notified && !consumed {
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
            } else if consumed {
                "already-consumed"
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
    match current_master_holder(server, state) {
        Ok(Some(master)) if master == worker_id => Ok(()),
        Ok(Some(_)) => Err(Resp::err(
            "master authority required; ask the registered master to delegate",
        )),
        Ok(None) => Err(Resp::err(
            "no master is assigned; a peer may promote itself only with explicit user approval",
        )),
        Err(error) => Err(Resp::err(error)),
    }
}

/// Clear the current typed grant for this route.
///
/// Clear needs no incumbent probe and does not require the caller to be the
/// current master. It runs the same authenticated admission as promote: the
/// caller's token and current binding are already proven by
/// `project_route_actor`, and the approval text is the human authorization. The
/// revoke set is the same scoped owner promote uses, minus the re-grant, so a
/// cleared route cannot be resurrected by legacy authority state.
const MASTER_WAKE_HOLD_MAX_REASON_CHARS: usize = 2000;
const MASTER_WAKE_HOLD_MAX_TTL_SECONDS: u64 = 604_800;

/// Record one explicit master-idle wake hold for a genuine external gate.
///
/// The hold is durable guard state, not a message: it only suppresses periodic
/// master-idle emissions while `now < until_ms`. A repeat hold replaces the
/// previous gate. Expiry is derived from `until_ms`; no clearing timer exists.
fn handle_master_wake_hold(
    server: &Server,
    worker_id: String,
    token: String,
    reason: String,
    ttl_seconds: u64,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify_master_actor(server, &state, &worker_id, &token) {
        return error;
    }
    if reason.trim().is_empty() {
        return Resp::err(
            "MASTER_WAKE_HOLD_REASON_EMPTY: reason must describe the gate and its release condition",
        );
    }
    if reason.chars().any(char::is_control) {
        return Resp::err(
            "MASTER_WAKE_HOLD_REASON_CONTROL_CHAR: reason must not contain control characters",
        );
    }
    if reason.chars().count() > MASTER_WAKE_HOLD_MAX_REASON_CHARS {
        return Resp::err(
            "MASTER_WAKE_HOLD_REASON_TOO_LONG: reason must be at most 2000 characters",
        );
    }
    if ttl_seconds == 0 || ttl_seconds > MASTER_WAKE_HOLD_MAX_TTL_SECONDS {
        return Resp::err(
            "MASTER_WAKE_HOLD_TTL_INVALID: ttl-seconds must be within 1..=604800",
        );
    }
    let held_ms = now_ms();
    let until_ms = held_ms.saturating_add((ttl_seconds as i64).saturating_mul(1000));
    let reason = reason.trim().to_string();
    let mut accumulator = state.master_wake.clone();
    let replaced = accumulator.wake_hold.is_some();
    accumulator.wake_hold = Some(notification_state::WakeHold {
        reason: reason.clone(),
        held_by: worker_id.clone(),
        held_ms,
        until_ms,
    });
    if let Err(error) = server.commit_locked_checked(
        &mut state,
        &[Event::MasterWakeUpdated {
            accumulator: accumulator.clone(),
        }],
    ) {
        return Resp::err(format!("MASTER_WAKE_HOLD_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "held": true,
        "replaced": replaced,
        "held_by": worker_id,
        "held_ms": held_ms,
        "until_ms": until_ms,
        "reason": reason,
        "master_wake": accumulator,
    }))
}

/// Clear the explicit wake hold. Idempotent: no hold is a successful no-op.
fn handle_master_wake_release(server: &Server, worker_id: String, token: String) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify_master_actor(server, &state, &worker_id, &token) {
        return error;
    }
    let mut accumulator = state.master_wake.clone();
    let was_held = accumulator.wake_hold.take().is_some();
    if was_held {
        if let Err(error) = server.commit_locked_checked(
            &mut state,
            &[Event::MasterWakeUpdated {
                accumulator: accumulator.clone(),
            }],
        ) {
            return Resp::err(format!("MASTER_WAKE_RELEASE_DURABILITY_FAILED: {error}"));
        }
    }
    Resp::data(json!({
        "released": true,
        "was_held": was_held,
        "held_by": worker_id,
        "master_wake": accumulator,
    }))
}

include!("part_07/worker_close.rs");

fn same_selected_endpoint(left: &crate::proto::SelectedTransport, right: &crate::proto::SelectedTransport) -> bool {
    left.kind == right.kind
        && left.endpoint == right.endpoint
        && left.namespace == right.namespace
        && left.session_id == right.session_id
        && left.thread_id == right.thread_id
        && left.tmux_endpoint == right.tmux_endpoint
        && left.rex_endpoint == right.rex_endpoint
}

fn worker_status_agent_session_id(transport: &crate::proto::SelectedTransport) -> Option<&str> {
    transport
        .session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            transport
                .tmux_endpoint
                .as_ref()
                .and_then(|endpoint| endpoint.codex_session_id.as_deref())
                .filter(|value| !value.trim().is_empty())
        })
}

fn worker_status_records(
    state: &State,
    project_scope: &str,
    session_id: &str,
    worker_id: &str,
) -> (
    Option<name_state::AgentPresenceRecord>,
    Option<name_state::AgentActivityRecord>,
) {
    (
        state
            .agent_presence
            .get(project_scope)
            .and_then(|workers| workers.get(worker_id))
            .filter(|record| record.session_id == session_id)
            .cloned(),
        state
            .agent_activity
            .get(project_scope)
            .and_then(|workers| workers.get(worker_id))
            .filter(|record| record.session_id == session_id)
            .cloned(),
    )
}

fn worker_status_capture(
    server: &Server,
    control: &binding_slot::BindingControlSet,
    state: &State,
    target_id: &str,
    captured_ms: i64,
) -> Result<crate::proto::WorkerStatusCapture, String> {
    let route = server_route_scope(server, state)
        .map_err(str::to_owned)?
        .ok_or_else(|| "WORKER_STATUS_CAPTURE_ROUTE_UNKNOWN".to_owned())?;
    let target = state
        .workers
        .get(target_id)
        .cloned()
        .ok_or_else(|| format!("target worker {target_id} not registered"))?;
    let selected_transport = target
        .transport
        .clone()
        .ok_or_else(|| format!("worker {target_id} has no registered transport"))?;
    let binding = current_worker_binding(
        state,
        &route.project_scope_id,
        Some(&route.app_scope_id),
        &target,
    )
    .cloned()
    .ok_or_else(|| "WORKER_STATUS_CAPTURE_BINDING_CHANGED".to_owned())?;
    let slot = control
        .current_for_worker(
            &route.project_scope_id,
            &route.app_scope_id,
            target_id,
        )?
        .ok_or_else(|| "WORKER_STATUS_CAPTURE_NAME_NOT_CURRENT".to_owned())?;
    let active = slot
        .active_binding()
        .ok_or_else(|| "WORKER_STATUS_CAPTURE_NAME_NOT_CURRENT".to_owned())?;
    if active.binding != binding || !same_selected_endpoint(&active.transport, &selected_transport) {
        return Err("WORKER_STATUS_CAPTURE_TARGET_CHANGED".into());
    }
    let agent_session_id = worker_status_agent_session_id(&selected_transport)
        .map(str::to_owned)
        .ok_or_else(|| "WORKER_STATUS_CAPTURE_SESSION_UNKNOWN".to_owned())?;
    let (presence, activity) = worker_agent_facts(server, state, &target);
    let missing_fields = name_monitor::missing_status_fields(presence, activity);
    let session_probe = if missing_fields.is_empty() {
        crate::proto::WorkerStatusSessionProbe::NotRequired
    } else {
        crate::proto::WorkerStatusSessionProbe::Unsupported
    };
    let master_worker_id = current_master_grant(state, Some(&route))
        .map(|grant| grant.agent_id.as_str().to_owned());
    Ok(crate::proto::WorkerStatusCapture {
        project_scope: route.project_scope_id.as_str().to_owned(),
        app_scope_id: route.app_scope_id.as_str().to_owned(),
        name: slot.identity_key.name.clone(),
        binding_generation: slot.generation,
        worker_id: target_id.to_owned(),
        agent_session_id,
        binding,
        selected_transport,
        presence: presence.cloned(),
        activity: activity.cloned(),
        missing_fields,
        session_probe,
        master_worker_id,
        captured_ms,
    })
}

fn worker_status_session_probe(
    server: &Server,
    capture: &mut crate::proto::WorkerStatusCapture,
    target: &WorkerRec,
) -> crate::proto::WorkerStatusSessionProbe {
    use crate::proto::WorkerStatusSessionProbe;

    if capture.missing_fields.is_empty() {
        capture.session_probe = WorkerStatusSessionProbe::NotRequired;
        return capture.session_probe;
    }
    if !matches!(
        capture.selected_transport.kind,
        TransportKind::AppServer | TransportKind::DesktopBridge | TransportKind::Dsh
    ) {
        capture.session_probe = WorkerStatusSessionProbe::Unsupported;
        return capture.session_probe;
    }

    let missing_before = capture.missing_fields.clone();
    let now = now_ms();
    let native_activity = keepalive::current_board_patrol_activity(server, target)
        .ok()
        .unwrap_or(name_state::AgentActivity::Unknown);
    let mut applied = false;

    if missing_before.contains(&crate::proto::WorkerStatusField::Activity)
        && native_activity != name_state::AgentActivity::Unknown
    {
        let observed_ms = next_status_observation_ms(
            now,
            capture.activity.as_ref().map(|record| record.observed_ms),
        );
        capture.activity = Some(name_state::AgentActivityRecord {
            project_scope: capture.project_scope.clone(),
            worker_id: capture.worker_id.clone(),
            session_id: capture.agent_session_id.clone(),
            activity: native_activity,
            background: false,
            reason: "session status capability: current native thread".into(),
            observed_ms,
            idle_since_ms: (native_activity == name_state::AgentActivity::Idle)
                .then_some(observed_ms),
        });
        applied = true;
    }

    if missing_before.contains(&crate::proto::WorkerStatusField::Presence) {
        let presence = if native_activity != name_state::AgentActivity::Unknown {
            Some(name_state::AgentPresence::Online)
        } else {
            let status_presence = match capture.selected_transport.kind {
                // The DSH gateway reports agent facts, not an AppServer thread
                // view, so reuse its existing presence mapping.
                TransportKind::Dsh => dsh_identity_presence(&capture.selected_transport),
                _ => capture
                    .selected_transport
                    .thread_id
                    .as_deref()
                    .map(|thread_id| {
                        thread_status_presence((server.appserver_thread_status)(
                            &capture.selected_transport,
                            thread_id,
                        ))
                    })
                    .unwrap_or(IdentityPresence::Unknown),
            };
            match status_presence {
                IdentityPresence::Present => Some(name_state::AgentPresence::Online),
                IdentityPresence::Missing => Some(name_state::AgentPresence::Offline),
                IdentityPresence::Cold | IdentityPresence::Unknown => None,
            }
        };
        if let Some(presence) = presence {
            let observed_ms = next_status_observation_ms(
                now,
                capture
                    .presence
                    .as_ref()
                    .map(|record| record.observed_ms),
            );
            capture.presence = Some(name_state::AgentPresenceRecord {
                project_scope: capture.project_scope.clone(),
                worker_id: capture.worker_id.clone(),
                session_id: capture.agent_session_id.clone(),
                presence,
                reason: if presence == name_state::AgentPresence::Offline {
                    "session status capability: target thread is absent".into()
                } else {
                    "session status capability: current native thread".into()
                },
                observed_ms,
            });
            applied = true;
        }
    }

    capture.missing_fields = name_monitor::missing_status_fields(
        capture.presence.as_ref(),
        capture.activity.as_ref(),
    );
    capture.session_probe = if applied {
        WorkerStatusSessionProbe::Applied
    } else {
        WorkerStatusSessionProbe::Inconclusive
    };
    capture.session_probe
}

fn commit_worker_status_session_probe(
    server: &Server,
    control: &binding_slot::BindingControlSet,
    baseline: &crate::proto::WorkerStatusCapture,
    probed: &crate::proto::WorkerStatusCapture,
) -> Result<(), Resp> {
    let mut state = server.state.lock().unwrap();
    let current = match worker_status_capture(
        server,
        control,
        &state,
        &baseline.worker_id,
        now_ms(),
    ) {
        Ok(current) => current,
        Err(error) => return Err(Resp::err(error)),
    };
    if current.name != baseline.name
        || current.binding_generation != baseline.binding_generation
        || current.agent_session_id != baseline.agent_session_id
        || current.binding != baseline.binding
        || current.selected_transport != baseline.selected_transport
        || current.presence != baseline.presence
        || current.activity != baseline.activity
        || current.missing_fields != baseline.missing_fields
    {
        return Err(Resp::err(
            "WORKER_STATUS_CAPTURE_STALE: target changed after the session status probe",
        ));
    }

    let mut events = Vec::new();
    if baseline
        .missing_fields
        .contains(&crate::proto::WorkerStatusField::Presence)
    {
        if let Some(presence) = probed.presence.clone() {
            events.push(Event::AgentPresenceObserved { presence });
        }
    }
    if baseline
        .missing_fields
        .contains(&crate::proto::WorkerStatusField::Activity)
    {
        if let Some(activity) = probed.activity.clone() {
            events.push(Event::AgentActivityObserved { activity });
        }
    }
    if !events.is_empty() {
        if let Err(error) = server.commit_locked_checked_with_control(&mut state, &events, control) {
            return Err(Resp::err(format!(
                "WORKER_STATUS_SESSION_PROBE_DURABILITY_FAILED: {error}"
            )));
        }
    }
    Ok(())
}

fn worker_status_judgment_receipt(
    state: &State,
    capture: &crate::proto::WorkerStatusCapture,
    outcome: crate::proto::WorkerStatusJudgmentOutcome,
    applied_fields: Vec<crate::proto::WorkerStatusField>,
    rejected_fields: Vec<crate::proto::WorkerStatusField>,
    reason: String,
) -> Resp {
    let (presence, activity) = worker_status_records(
        state,
        &capture.project_scope,
        &capture.agent_session_id,
        &capture.worker_id,
    );
    Resp::data(
        serde_json::to_value(crate::proto::WorkerStatusJudgmentReceipt {
            outcome,
            applied_fields,
            rejected_fields,
            reason,
            presence,
            activity,
        })
        .expect("worker status judgment receipt is serializable"),
    )
}

fn next_status_observation_ms(now: i64, previous: Option<i64>) -> i64 {
    previous.map_or(now, |observed_ms| now.max(observed_ms.saturating_add(1)))
}

fn worker_status_capture_response(
    capture: &crate::proto::WorkerStatusCapture,
    snapshot_collected: bool,
    mut snapshot: serde_json::Value,
) -> Resp {
    snapshot["worker_id"] = capture.worker_id.clone().into();
    snapshot["judgment_capture"] =
        serde_json::to_value(capture).expect("worker status capture is serializable");
    snapshot["session_probe"] = serde_json::to_value(capture.session_probe)
        .expect("worker status session probe is serializable");
    snapshot["snapshot_collected"] = snapshot_collected.into();
    let snapshot_unavailable = snapshot
        .get("snapshot")
        .and_then(serde_json::Value::as_str)
        == Some("not_supported");
    snapshot["judgment_status"] = if capture.missing_fields.is_empty() {
        json!("satisfied")
    } else if snapshot_unavailable {
        json!("unknown")
    } else if snapshot_collected && capture.master_worker_id.is_some() {
        json!("ready")
    } else if !snapshot_collected {
        json!("unknown")
    } else {
        json!("awaiting_master")
    };
    Resp::data(snapshot)
}

#[allow(clippy::too_many_arguments)]
fn handle_worker_status_judgment(
    server: &Server,
    state: &mut State,
    control: &binding_slot::BindingControlSet,
    worker_id: &str,
    token: &str,
    capture: &crate::proto::WorkerStatusCapture,
    presence_judgment: Option<name_state::AgentPresence>,
    activity_judgment: Option<name_state::AgentActivity>,
    basis: &str,
) -> Resp {
    let current_capture_records = worker_status_records(
        state,
        &capture.project_scope,
        &capture.agent_session_id,
        &capture.worker_id,
    );
    if state
        .workers
        .get(worker_id)
        .is_none_or(|worker| worker.token != token)
    {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::Unauthorized,
            Vec::new(),
            Vec::new(),
            "the caller is not the authenticated target master".into(),
        );
    }
    let route = match server_route_scope(server, state) {
        Ok(Some(route)) => route,
        Ok(None) => {
            return worker_status_judgment_receipt(
                state,
                capture,
                crate::proto::WorkerStatusJudgmentOutcome::StaleTarget,
                Vec::new(),
                Vec::new(),
                "the current project route is no longer available".into(),
            )
        }
        Err(error) => return Resp::err(error),
    };
    if capture.project_scope != route.project_scope_id.as_str()
        || capture.app_scope_id != route.app_scope_id.as_str()
    {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::StaleTarget,
            Vec::new(),
            Vec::new(),
            "the capture belongs to another project or app scope".into(),
        );
    }
    let Some(target) = state.workers.get(&capture.worker_id).cloned() else {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::StaleTarget,
            Vec::new(),
            Vec::new(),
            "the captured worker is no longer current".into(),
        );
    };
    let current_binding = current_worker_binding(
        state,
        &route.project_scope_id,
        Some(&route.app_scope_id),
        &target,
    );
    if current_binding != Some(&capture.binding)
        || target.transport.as_ref() != Some(&capture.selected_transport)
    {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::StaleTarget,
            Vec::new(),
            Vec::new(),
            "the captured holder, session, binding, or selected endpoint is no longer current".into(),
        );
    }
    let Some(current_agent_session_id) =
        worker_status_agent_session_id(&capture.selected_transport)
    else {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::StaleTarget,
            Vec::new(),
            Vec::new(),
            "the captured agent session is no longer current".into(),
        );
    };
    if capture.agent_session_id != current_agent_session_id {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::StaleTarget,
            Vec::new(),
            Vec::new(),
            "the captured agent session is no longer current".into(),
        );
    }
    let slot = match control.current_for_worker(
        &route.project_scope_id,
        &route.app_scope_id,
        &capture.worker_id,
    ) {
        Ok(Some(slot)) => slot,
        Ok(None) => {
            return worker_status_judgment_receipt(
                state,
                capture,
                crate::proto::WorkerStatusJudgmentOutcome::StaleTarget,
                Vec::new(),
                Vec::new(),
                "the captured stable name is no longer current".into(),
            )
        }
        Err(error) => return Resp::err(error),
    };
    if slot.identity_key.name != capture.name
        || slot.generation != capture.binding_generation
        || slot.active_binding().is_none_or(|active| {
            active.binding != capture.binding
                || !same_selected_endpoint(&active.transport, &capture.selected_transport)
        })
    {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::StaleTarget,
            Vec::new(),
            Vec::new(),
            "the captured stable-name generation or binding changed".into(),
        );
    }
    if current_capture_records != (capture.presence.clone(), capture.activity.clone()) {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::StaleFacts,
            Vec::new(),
            Vec::new(),
            "native presence or activity changed after the snapshot capture".into(),
        );
    }
    let missing_fields = name_monitor::missing_status_fields(
        current_capture_records.0.as_ref(),
        current_capture_records.1.as_ref(),
    );
    if missing_fields != capture.missing_fields {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::StaleFacts,
            Vec::new(),
            Vec::new(),
            "the capture's missing-field baseline no longer matches current facts".into(),
        );
    }
    let probe_matches_missing = if missing_fields.is_empty() {
        capture.session_probe == crate::proto::WorkerStatusSessionProbe::NotRequired
    } else {
        capture.session_probe != crate::proto::WorkerStatusSessionProbe::NotRequired
    };
    if !probe_matches_missing {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::StaleFacts,
            Vec::new(),
            Vec::new(),
            "the capture's session-probe result no longer matches its missing fields".into(),
        );
    }
    if missing_fields.is_empty() {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::Satisfied,
            Vec::new(),
            Vec::new(),
            "captured status facts are already sufficient".into(),
        );
    }

    let Some(master) = current_master_grant(state, Some(&route)) else {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::AwaitingMaster,
            Vec::new(),
            Vec::new(),
            "the project has no current master grant".into(),
        );
    };
    let caller_binding = current_worker_binding(
        state,
        &route.project_scope_id,
        Some(&route.app_scope_id),
        match state.workers.get(worker_id) {
            Some(worker) => worker,
            None => {
                return worker_status_judgment_receipt(
                    state,
                    capture,
                    crate::proto::WorkerStatusJudgmentOutcome::Unauthorized,
                    Vec::new(),
                    Vec::new(),
                    "the caller is no longer registered".into(),
                )
            }
        },
    );
    if master.agent_id.as_str() != worker_id
        || caller_binding.is_none_or(|binding| binding.binding_id != master.binding_id)
    {
        return worker_status_judgment_receipt(
            state,
            capture,
            crate::proto::WorkerStatusJudgmentOutcome::Unauthorized,
            Vec::new(),
            Vec::new(),
            "the caller is not the current project master".into(),
        );
    }
    if basis.trim().is_empty() {
        return Resp::err("WORKER_STATUS_JUDGMENT_BASIS_REQUIRED");
    }
    if presence_judgment.is_none() && activity_judgment.is_none() {
        return Resp::err("WORKER_STATUS_JUDGMENT_EMPTY");
    }

    let presence_missing = missing_fields.contains(&crate::proto::WorkerStatusField::Presence);
    let activity_missing = missing_fields.contains(&crate::proto::WorkerStatusField::Activity);
    let mut events = Vec::new();
    let mut applied_fields = Vec::new();
    let mut rejected_fields = Vec::new();
    let mut rejection_reasons: Vec<String> = Vec::new();
    let now = now_ms();

    if let Some(judgment) = presence_judgment {
        if !presence_missing {
            rejected_fields.push(crate::proto::WorkerStatusField::Presence);
            rejection_reasons.push("presence is already determined".into());
        } else if judgment != name_state::AgentPresence::Unknown {
            let observed_ms = next_status_observation_ms(
                now,
                current_capture_records
                    .0
                    .as_ref()
                    .map(|record| record.observed_ms),
            );
            events.push(Event::AgentPresenceObserved {
                presence: name_state::AgentPresenceRecord {
                    project_scope: capture.project_scope.clone(),
                    worker_id: capture.worker_id.clone(),
                    session_id: capture.agent_session_id.clone(),
                    presence: judgment,
                    reason: format!("master snapshot judgment: {}", basis.trim()),
                    observed_ms,
                },
            });
            applied_fields.push(crate::proto::WorkerStatusField::Presence);
        }
    }

    if let Some(judgment) = activity_judgment {
        if !activity_missing {
            rejected_fields.push(crate::proto::WorkerStatusField::Activity);
            rejection_reasons.push("activity is already determined".into());
        } else if judgment == name_state::AgentActivity::Unknown {
            // Unknown is a valid conclusion but writes no fabricated state.
        } else {
            let online = presence_judgment == Some(name_state::AgentPresence::Online)
                || current_capture_records
                    .0
                    .as_ref()
                    .is_some_and(|record| record.presence == name_state::AgentPresence::Online);
            if !online {
                rejected_fields.push(crate::proto::WorkerStatusField::Activity);
                rejection_reasons.push("activity requires Online presence".into());
            } else {
                let observed_ms = next_status_observation_ms(
                    now,
                    current_capture_records
                        .1
                        .as_ref()
                        .map(|record| record.observed_ms),
                );
                events.push(Event::AgentActivityObserved {
                    activity: name_state::AgentActivityRecord {
                        project_scope: capture.project_scope.clone(),
                        worker_id: capture.worker_id.clone(),
                        session_id: capture.agent_session_id.clone(),
                        activity: judgment,
                        background: false,
                        reason: format!("master snapshot judgment: {}", basis.trim()),
                        observed_ms,
                        // A single master judgment establishes the current
                        // activity, not how long that activity has continued.
                        idle_since_ms: None,
                    },
                });
                applied_fields.push(crate::proto::WorkerStatusField::Activity);
            }
        }
    }

    if !events.is_empty() {
        if let Err(error) = server.commit_locked_checked_with_control(state, &events, control) {
            return Resp::err(format!(
                "WORKER_STATUS_JUDGMENT_DURABILITY_FAILED: {error}"
            ));
        }
    }

    let updated_records = worker_status_records(
        state,
        &capture.project_scope,
        &capture.agent_session_id,
        &capture.worker_id,
    );
    let unresolved_fields = name_monitor::missing_status_fields(
        updated_records.0.as_ref(),
        updated_records.1.as_ref(),
    )
        .into_iter()
        .filter(|field| !rejected_fields.contains(field))
        .collect::<Vec<_>>();
    let outcome = if applied_fields.is_empty() && rejected_fields.is_empty() {
        crate::proto::WorkerStatusJudgmentOutcome::Unknown
    } else if applied_fields.is_empty() {
        crate::proto::WorkerStatusJudgmentOutcome::Rejected
    } else if rejected_fields.is_empty() && unresolved_fields.is_empty() {
        crate::proto::WorkerStatusJudgmentOutcome::Applied
    } else {
        crate::proto::WorkerStatusJudgmentOutcome::Partial
    };
    let reason = if rejection_reasons.is_empty() && unresolved_fields.is_empty() {
        format!("master snapshot judgment applied: {}", basis.trim())
    } else {
        let mut reasons = rejection_reasons;
        if !unresolved_fields.is_empty() {
            reasons.push(format!(
                "still missing fields: {}",
                unresolved_fields
                    .iter()
                    .map(|field| format!("{field:?}").to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        format!("master snapshot judgment incomplete: {}", reasons.join("; "))
    };
    worker_status_judgment_receipt(
        state,
        capture,
        outcome,
        applied_fields,
        rejected_fields,
        reason,
    )
}

fn handle_worker_snapshot(
    server: &Server,
    worker_id: String,
    token: String,
    target_id: String,
    lines: usize,
) -> Resp {
    let control =
        match crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root())
            .load()
        {
            Ok(control) => control,
            Err(error) => return Resp::err(error),
        };
    let state = server.state.lock().unwrap();
    // A waiting peer observes the peer it waits for without disturbing it, so
    // this read is admitted for any authenticated peer of this exact project
    // route. Master authority is not the subject of a transient pane read and
    // must not gate it.
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
    if !(1..=200).contains(&lines) {
        return Resp::err("snapshot lines must be 1..200");
    }
    let target = match state.workers.get(&target_id).cloned() {
        Some(target) => target,
        None => return Resp::err(format!("target worker {target_id} not registered")),
    };
    let capture = match worker_status_capture(server, &control, &state, &target_id, now_ms()) {
        Ok(capture) => capture,
        Err(error) => return Resp::err(error),
    };
    if capture.missing_fields.is_empty() {
        drop(state);
        return worker_status_capture_response(
            &capture,
            false,
            serde_json::json!({
                "snapshot": "not_collected",
                "reason": "current status facts are already sufficient",
            }),
        );
    }
    drop(state);

    let mut capture = capture;
    let baseline = capture.clone();
    let session_probe = worker_status_session_probe(server, &mut capture, &target);
    if session_probe == crate::proto::WorkerStatusSessionProbe::Applied {
        if let Err(response) =
            commit_worker_status_session_probe(server, &control, &baseline, &capture)
        {
            return response;
        }
    }
    if capture.missing_fields.is_empty() {
        return worker_status_capture_response(
            &capture,
            false,
            serde_json::json!({
                "snapshot": "not_collected",
                "reason": if session_probe == crate::proto::WorkerStatusSessionProbe::Applied {
                    "session status capability resolved the current status facts"
                } else {
                    "current status facts are already sufficient"
                },
            }),
        );
    }
    if !matches!(
        capture.selected_transport.kind,
        crate::proto::TransportKind::Tmux | crate::proto::TransportKind::Rex
    ) {
        return worker_status_capture_response(
            &capture,
            false,
            serde_json::json!({
                "snapshot": "not_supported",
                "reason": match session_probe {
                    crate::proto::WorkerStatusSessionProbe::Unsupported => {
                        "the selected transport has no independent session status capability or snapshot"
                    }
                    _ => "the session status capability did not resolve the remaining facts and the selected transport has no snapshot",
                },
            }),
        );
    }

    enum SnapshotTarget {
        Tmux(crate::proto::TmuxEndpoint),
        Rex(crate::proto::RexEndpoint),
    }
    let target_endpoint = match capture.selected_transport.kind {
        crate::proto::TransportKind::Tmux => match capture.selected_transport.tmux_endpoint.clone() {
            Some(endpoint) => SnapshotTarget::Tmux(endpoint),
            None => {
                return Resp::err(format!(
                    "worker {target_id} has no registered tmux endpoint"
                ))
            }
        },
        crate::proto::TransportKind::Rex => match capture.selected_transport.rex_endpoint.clone() {
            Some(endpoint) => SnapshotTarget::Rex(endpoint),
            None => {
                return Resp::err(format!(
                    "worker {target_id} has no registered rex endpoint"
                ))
            }
        },
        crate::proto::TransportKind::DesktopBridge
        | crate::proto::TransportKind::AppServer
        | crate::proto::TransportKind::Dsh => {
            return Resp::err(
                "WORKER_SNAPSHOT_UNSUPPORTED: only registered tmux and rex peers expose transient pane buffers",
            )
        }
    };
    // Transport execution never runs under the reducer lock. The cloned
    // endpoint is the admitted route; the response describes that one capture,
    // not a later claim about the peer's state.
    let snapshot = match target_endpoint {
        SnapshotTarget::Tmux(endpoint) => {
            match crate::client::adapters::tmux::snapshot(&endpoint, lines) {
                Ok(snapshot) => snapshot,
                Err(error) => return Resp::err(error),
            }
        }
        SnapshotTarget::Rex(endpoint) => {
            // The daemon dispatches wire requests from the Tokio blocking
            // pool, so the current runtime handle is available for the Rex
            // primitive's async process I/O.
            let handle = match tokio::runtime::Handle::try_current() {
                Ok(handle) => handle,
                Err(error) => {
                    return Resp::err(format!(
                        "REX_UNKNOWN: Tokio runtime unavailable for Rex snapshot: {error}"
                    ))
                }
            };
            match handle.block_on(crate::client::adapters::rex::snapshot(&endpoint, lines)) {
                Ok(snapshot) => snapshot,
                Err(error) => return Resp::err(error.to_string()),
            }
        }
    };
    worker_status_capture_response(&capture, true, snapshot)
}

#[cfg(test)]
mod worker_status_judgment_tests {
    use super::*;
    use crate::server::binding_store::BindingControlStore;
    use crate::server::peer_tests;

    #[test]
    fn stale_activity_update_after_capture_rejects_same_millisecond_native_fact() {
        let (server, root) = peer_tests::test_server();
        assert!(
            peer_tests::register_synthetic_peer(
                &server,
                "status-master",
                "thread-status-master"
            )
            .ok
        );
        assert!(
            peer_tests::register_synthetic_peer(&server, "status-peer", "thread-status-peer").ok
        );
        peer_tests::promote_master(&server, "status-master", "same-ms judgment fixture");

        let (project_scope, session_id, master_token) = {
            let state = server.state.lock().unwrap();
            let target = state.workers.get("status-peer").unwrap();
            let transport = target.transport.as_ref().unwrap();
            let binding = state
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values())
                .find(|binding| binding.agent_id.as_str() == "status-peer")
                .expect("status peer binding");
            (
                binding.project_scope.as_str().to_owned(),
                worker_status_agent_session_id(transport)
                    .expect("status peer agent session")
                    .to_owned(),
                state.workers["status-master"].token.clone(),
            )
        };
        let activity_before = name_state::AgentActivityRecord {
            project_scope: project_scope.clone(),
            worker_id: "status-peer".into(),
            session_id: session_id.clone(),
            activity: name_state::AgentActivity::Unknown,
            background: false,
            reason: "baseline before snapshot".into(),
            observed_ms: 4_242,
            idle_since_ms: None,
        };
        server
            .try_commit(&[Event::AgentActivityObserved {
                activity: activity_before,
            }])
            .expect("seed baseline activity");

        let control = BindingControlStore::new(server.host_paths.state_root())
            .load()
            .expect("load fixture control");
        let capture = {
            let state = server.state.lock().unwrap();
            worker_status_capture(&server, &control, &state, "status-peer", 4_242)
                .expect("capture status target")
        };
        assert_eq!(
            capture.activity.as_ref().map(|record| record.activity),
            Some(name_state::AgentActivity::Unknown)
        );

        let project_scope_check = project_scope.clone();
        server
            .try_commit(&[Event::AgentActivityObserved {
                activity: name_state::AgentActivityRecord {
                    project_scope,
                    worker_id: "status-peer".into(),
                    session_id,
                    activity: name_state::AgentActivity::Busy,
                    background: false,
                    reason: "native update at the capture millisecond".into(),
                    observed_ms: 4_242,
                    idle_since_ms: None,
                },
            }])
            .expect("commit same-millisecond native update");

        let receipt = {
            let mut state = server.state.lock().unwrap();
            handle_worker_status_judgment(
                &server,
                &mut state,
                &control,
                "status-master",
                &master_token,
                &capture,
                Some(name_state::AgentPresence::Online),
                Some(name_state::AgentActivity::Busy),
                "late snapshot judgment",
            )
        };
        assert!(receipt.ok, "{receipt:?}");
        assert_eq!(receipt.data["outcome"], "stale_facts", "{receipt:?}");
        assert_eq!(receipt.data["activity"]["activity"], "busy", "{receipt:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.agent_activity[&project_scope_check]["status-peer"].activity,
            name_state::AgentActivity::Busy
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A private DSH control plane for one status pair. It answers the two
    /// read-only methods the session status capability already uses.
    struct DshStatusGateway {
        dir: std::path::PathBuf,
        socket: std::path::PathBuf,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl DshStatusGateway {
        fn start(label: &str, status: &'static str, live: bool) -> Self {
            use std::io::{BufRead, BufReader, Write};
            use std::os::unix::net::UnixListener;
            use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

            static SEQ: AtomicU64 = AtomicU64::new(0);
            let id = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "collab-dsh-status-{label}-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("create dsh status fixture");
            let socket = dir.join("control.sock");
            let listener = UnixListener::bind(&socket).expect("bind dsh status socket");
            let stop = std::sync::Arc::new(AtomicBool::new(false));
            let flag = std::sync::Arc::clone(&stop);
            let cwd = dir.display().to_string();
            let handle = std::thread::spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    if flag.load(Ordering::SeqCst) {
                        return;
                    }
                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        continue;
                    }
                    let request: serde_json::Value =
                        serde_json::from_str(line.trim()).expect("dsh status request is JSON");
                    let reply = match request["method"].as_str().unwrap_or_default() {
                        "agent-get" => serde_json::json!({
                            "ok": true,
                            "result": {
                                "runtimeId": "rt-1",
                                "ok": true,
                                "result": {
                                    "agent": {
                                        "agentId": "ses-1",
                                        "status": status,
                                        "live": live,
                                    }
                                }
                            }
                        }),
                        "agent-facts" => {
                            let nonce = request
                                .pointer("/params/nonce")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default();
                            serde_json::json!({
                                "ok": true,
                                "result": {
                                    "nonce": nonce,
                                    "runtimeId": "rt-1",
                                    "agentId": "ses-1",
                                    "sessionId": "ses-1",
                                    "cwd": cwd.clone(),
                                    "status": status,
                                }
                            })
                        }
                        other => panic!("unexpected dsh status method {other}"),
                    };
                    let mut stream = &stream;
                    let _ = stream.write_all(reply.to_string().as_bytes());
                    let _ = stream.write_all(b"\n");
                    let _ = stream.flush();
                }
            });
            Self {
                dir,
                socket,
                stop,
                handle: Some(handle),
            }
        }

        fn endpoint(&self) -> String {
            format!("unix://{}", self.socket.display())
        }
    }

    impl Drop for DshStatusGateway {
        fn drop(&mut self) {
            use std::sync::atomic::Ordering;

            self.stop.store(true, Ordering::SeqCst);
            let _ = std::os::unix::net::UnixStream::connect(&self.socket);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn dsh_session_capability_resolves_live_status_without_snapshot() {
        use crate::proto::{SelectedTransport, TransportKind, WorkerStatusSessionProbe};

        for (label, status, live, activity, presence, probe) in [
            (
                "running",
                "running",
                true,
                Some(name_state::AgentActivity::Busy),
                Some(name_state::AgentPresence::Online),
                WorkerStatusSessionProbe::Applied,
            ),
            (
                "inactive",
                "inactive",
                true,
                Some(name_state::AgentActivity::Idle),
                Some(name_state::AgentPresence::Online),
                WorkerStatusSessionProbe::Applied,
            ),
            (
                "not-live",
                "inactive",
                false,
                None,
                None,
                WorkerStatusSessionProbe::Inconclusive,
            ),
        ] {
            let (server, root) = peer_tests::test_server();
            let gateway = DshStatusGateway::start(label, status, live);
            let transport = SelectedTransport {
                kind: TransportKind::Dsh,
                endpoint: Some(gateway.endpoint()),
                namespace: Some("rt-1".into()),
                session_id: Some("ses-1".into()),
                thread_id: Some("ses-1".into()),
                tmux_endpoint: None,
                rex_endpoint: None,
                capabilities: vec!["agent-get".into(), "agent-facts".into()],
                self_check: "isolated dsh status fixture".into(),
            };
            let seeded = peer_tests::seed_committed_owner(
                &server,
                "status-dsh",
                "token-status-dsh",
                &transport,
                &root.display().to_string(),
                Some(crate::identity::AppServerId::new("tui-default").unwrap()),
            );
            assert!(seeded.ok, "{label}: {seeded:?}");
            let control = BindingControlStore::new(server.host_paths.state_root())
                .load()
                .expect("load fixture control");
            let mut capture = {
                let state = server.state.lock().unwrap();
                worker_status_capture(&server, &control, &state, "status-dsh", now_ms())
                    .expect("capture dsh target")
            };
            assert_eq!(
                capture.missing_fields,
                vec![
                    crate::proto::WorkerStatusField::Presence,
                    crate::proto::WorkerStatusField::Activity,
                ],
                "{label}"
            );
            let target = server.state.lock().unwrap().workers["status-dsh"].clone();
            let actual = worker_status_session_probe(&server, &mut capture, &target);
            assert_eq!(actual, probe, "{label}: {capture:?}");
            assert_eq!(
                capture.activity.as_ref().map(|record| record.activity),
                activity,
                "{label}"
            );
            assert_eq!(
                capture.presence.as_ref().map(|record| record.presence),
                presence,
                "{label}"
            );
            if probe == WorkerStatusSessionProbe::Applied {
                assert!(capture.missing_fields.is_empty(), "{label}");
            } else {
                assert_eq!(
                    capture.missing_fields,
                    vec![
                        crate::proto::WorkerStatusField::Presence,
                        crate::proto::WorkerStatusField::Activity,
                    ],
                    "{label}"
                );
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn dsh_worker_snapshot_stops_after_the_session_capability() {
        use crate::proto::{SelectedTransport, TransportKind};

        let (server, root) = peer_tests::test_server();
        let gateway = DshStatusGateway::start("snapshot-stop", "running", true);
        let transport = SelectedTransport {
            kind: TransportKind::Dsh,
            endpoint: Some(gateway.endpoint()),
            namespace: Some("rt-1".into()),
            session_id: Some("ses-1".into()),
            thread_id: Some("ses-1".into()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec!["agent-get".into(), "agent-facts".into()],
            self_check: "isolated dsh snapshot fixture".into(),
        };
        let seeded = peer_tests::seed_committed_owner(
            &server,
            "status-dsh",
            "token-status-dsh",
            &transport,
            &root.display().to_string(),
            Some(crate::identity::AppServerId::new("tui-default").unwrap()),
        );
        assert!(seeded.ok, "{seeded:?}");

        let response = handle_worker_snapshot(
            &server,
            "status-dsh".into(),
            "token-status-dsh".into(),
            "status-dsh".into(),
            20,
        );
        assert!(response.ok, "{response:?}");
        assert_eq!(response.data["snapshot"], "not_collected", "{response:?}");
        assert_eq!(response.data["snapshot_collected"], false, "{response:?}");
        assert_eq!(response.data["session_probe"], "applied", "{response:?}");
        assert_eq!(response.data["judgment_status"], "satisfied", "{response:?}");
        assert_eq!(
            response.data["reason"], "session status capability resolved the current status facts",
            "{response:?}"
        );
        let state = server.state.lock().unwrap();
        let scope = crate::server::GlobalState::canonical_project_scope(&server.root).unwrap();
        assert_eq!(
            state.agent_presence[scope.as_str()]["status-dsh"].presence,
            name_state::AgentPresence::Online
        );
        assert_eq!(
            state.agent_activity[scope.as_str()]["status-dsh"].activity,
            name_state::AgentActivity::Busy
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
