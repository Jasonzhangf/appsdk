/// One authenticated, read-only capability projection.
///
/// Every branch reuses the existing owner for that capability. This handler
/// stops before recipient resolution, message construction, mailbox reads,
/// registration, presence writes, notification rearm, or adapter calls.
fn handle_capability_readiness(
    server: &Server,
    worker_id: String,
    token: String,
    command: crate::proto::CommandEnvelope,
    project_context: Option<&ProjectContext>,
) -> Resp {
    // Context: the pure serializer, not `handle_context` with its lease and
    // presence-edge writes. The payload is discarded after validation.
    let context = context_projection(server, worker_id.clone(), token.clone());
    if !context.ok {
        return context;
    }

    if let Err(error) = board_readiness_projection(server, &worker_id) {
        return Resp::err(error);
    }

    let (transport_kind, receive_owner) = {
        let state = server.state.lock().unwrap();
        let worker = match verify(&state, &worker_id, &token) {
            Ok(worker) => worker,
            Err(response) => return response,
        };
        let Some(transport) = selected_transport_for_worker(&worker) else {
            return Resp::err(
                "CAPABILITY_READINESS_RECEIVE_UNAVAILABLE: retained caller has no selected transport",
            );
        };
        if transport.kind == TransportKind::Dsh {
            // The DSH gateway owns receive and Ack. CLI Poll readiness must not
            // be demanded from a transport whose contract forbids it.
            (transport.kind, "gateway")
        } else {
            let Some(context) = project_context else {
                return Resp::err(
                    "PROJECT_CONTEXT_REQUIRED: receive readiness requires the request project context",
                );
            };
            if let Err(response) = project_route_actor_for_poll(&state, context, &worker_id, &token)
            {
                return response;
            }
            (transport.kind, "cli")
        }
    };

    {
        let state = server.state.lock().unwrap();
        let Some(context) = project_context else {
            return Resp::err(
                "PROJECT_CONTEXT_REQUIRED: send readiness requires the request project context",
            );
        };
        if let Err(response) = authenticated_send_admission(
            &state,
            &worker_id,
            &token,
            &command,
            Some(&context.app_scope_id),
        ) {
            return response;
        }
    }

    Resp::data(json!({
        "status": "ready",
        "context": "passed",
        "board": "passed",
        "receive": {
            "status": "passed",
            "owner": receive_owner,
        },
        "send": "passed",
        "transport": {
            "kind": transport_kind.as_str(),
            "receive_owner": receive_owner,
        }
    }))
}

/// Reuse the board serializer and visibility owner without the transport
/// observations `handle_board_show` performs for its display projection.
fn board_readiness_projection(server: &Server, viewer: &str) -> Result<(), String> {
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load()?;
    let state = server.state.lock().unwrap();
    let route = server_route_scope(server, &state)?;
    for task in state
        .tasks
        .values()
        .filter(|task| !matches!(task.status.as_str(), "closed" | "cancelled"))
    {
        board_task_view_with_master(&state, &control, task, Some(viewer), None, route.as_ref())?;
    }
    for worker in state
        .workers
        .values()
        .filter(|worker| !is_managed_subagent(&state, &worker.id))
    {
        worker_display_name(&control, route.as_ref(), &worker.id)?;
        worker_work_roles(&state, &control, route.as_ref(), &worker.id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{AgentId, BindingId, CommandId, OperationId, RuntimeId, SessionId};
    use crate::server::binding_slot::{BindingControlSet, BindingValue, IdentityBindingKey};
    use crate::server::binding_store::BindingControlStore;
    use std::sync::atomic::{AtomicU64, Ordering};

    const WORKER: &str = "readiness-worker";
    const TOKEN: &str = "readiness-token";
    const SESSION: &str = "readiness-session";
    const RUNTIME: &str = "readiness-runtime";
    const BINDING: &str = "readiness-binding";

    fn dsh_transport(session: &str) -> SelectedTransport {
        SelectedTransport {
            kind: TransportKind::Dsh,
            endpoint: Some("unix:///tmp/readiness-gateway.sock".into()),
            namespace: Some("readiness-gateway".into()),
            session_id: Some(session.into()),
            thread_id: Some("readiness-thread".into()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec!["send_message_to_thread".into(), "probe_thread".into()],
            self_check: "readiness gateway".into(),
        }
    }

    fn seed_ready_server(server: &Server, root: &Path) {
        let canonical = std::fs::canonicalize(root).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let route = RouteScope::for_registered_project(app.clone(), &canonical).unwrap();
        let session = SessionId::new(SESSION).unwrap();
        let binding = RuntimeBinding::new_with_session(
            route.project_scope_id.clone(),
            app.clone(),
            AgentId::new(WORKER).unwrap(),
            RuntimeId::new(RUNTIME).unwrap(),
            BindingId::new(BINDING).unwrap(),
            Some(session.clone()),
            None,
        )
        .unwrap();

        let mut state = server.state.lock().unwrap();
        state
            .apply_checked(&Event::GlobalProjectRegistered {
                registration: ProjectRegistration::new(route.project_scope_id.clone(), app.clone())
                    .unwrap(),
            })
            .unwrap();
        state
            .apply_checked(&Event::GlobalRuntimeBound {
                binding: binding.clone(),
            })
            .unwrap();
        let worker = WorkerRec {
            id: WORKER.into(),
            token: TOKEN.into(),
            cwd: canonical.to_string_lossy().into_owned(),
            registered_ms: 1,
            transport: Some(dsh_transport(SESSION)),
        };
        state.apply_checked(&Event::Registered { worker }).unwrap();
        let task = TaskRec {
            id: "readiness-task".into(),
            owner: WORKER.into(),
            created_by: WORKER.into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "working".into(),
            next_step: None,
            wait: None,
            isolation_mode: None,
            created_ms: 1,
            updated_ms: 1,
        };
        state.apply_checked(&Event::TaskCreated { task }).unwrap();
        state
            .apply_checked(&Event::BoardDetailsChanged {
                task_id: "readiness-task".into(),
                details: crate::board::BoardTaskDetails::legacy("readiness-task", false),
            })
            .unwrap();
        drop(state);

        let key = IdentityBindingKey::new(route.project_scope_id.clone(), app.clone(), "Alice");
        let mut control = BindingControlSet::new();
        control
            .commit(
                &key,
                BindingValue::new(binding, dsh_transport(SESSION)),
                Some(0),
            )
            .unwrap();
        let store = BindingControlStore::new(server.host_paths.state_root());
        store.save(&control).unwrap();
    }

    fn readiness_command(root: &Path) -> CommandEnvelope {
        let canonical = std::fs::canonicalize(root).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let route = RouteScope::for_registered_project(app, &canonical).unwrap();
        CommandEnvelope::new(
            CommandId::new("readiness-command").unwrap(),
            OperationId::new("readiness-operation").unwrap(),
            BindingId::new(BINDING).unwrap(),
            route,
            None,
            None,
            None,
            None,
        )
    }

    fn dsh_context(root: &Path) -> ProjectContext {
        let canonical = std::fs::canonicalize(root).unwrap();
        ProjectContext::for_registered_route(
            &canonical,
            &crate::identity::RuntimeIdentity {
                agent_id: AgentId::new(WORKER).unwrap(),
                runtime_id: RuntimeId::new(RUNTIME).unwrap(),
                appserver_id: AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
                binding_id: BindingId::new(BINDING).unwrap(),
                session_id: Some(SessionId::new(SESSION).unwrap()),
                native_thread_id: None,
            },
        )
        .unwrap()
    }

    fn test_server() -> (Server, PathBuf, PathBuf, PathBuf) {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "collab-readiness-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
        let host = root.join("host-state");
        std::fs::create_dir_all(&host).unwrap();
        let state_path = root.join(".agent-collab/server/state.json");
        current_state::save(&state_path, &State::default()).unwrap();
        let server = Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            state_path: state_path.clone(),
            host_paths: HostPaths::for_state_root(&host).unwrap(),
            state: Mutex::new(State::default()),
            appserver_candidate_check: Arc::new(|candidate| {
                Ok(SelectedTransport {
                    kind: TransportKind::AppServer,
                    endpoint: Some(candidate.endpoint.clone()),
                    namespace: Some(candidate.namespace.clone()),
                    session_id: Some(candidate.session_id.clone()),
                    thread_id: Some(candidate.thread_id.clone()),
                    tmux_endpoint: None,
                    rex_endpoint: None,
                    capabilities: vec!["send_message_to_thread".into()],
                    self_check: "test appserver".into(),
                })
            }),
            appserver_notification_sink: Arc::new(|_, _, _, _, _, _| {
                Ok(serde_json::json!({"accepted": true}))
            }),
            appserver_thread_status: Arc::new(|_, thread_id| {
                Ok(serde_json::json!({
                    "thread": {"id": thread_id, "status": {"type": "idle"}, "canAcceptDirectInput": true}
                }))
            }),
            appserver_thread_archive: Arc::new(|_, _| Ok(serde_json::json!({"archived": true}))),
            rex_monitor: Default::default(),
            mailbox_notify: tokio::sync::Notify::new(),
        };
        (server, root, state_path, host)
    }

    #[test]
    fn capability_readiness_reports_gateway_receive_ownership_without_mutation() {
        let (server, root, state_path, host) = test_server();
        seed_ready_server(&server, &root);
        let state_before = std::fs::read(&state_path).unwrap();
        let map_before = std::fs::read(host.join("identity-map.json")).unwrap();
        assert!(!map_before.is_empty());

        let response = handle_capability_readiness(
            &server,
            WORKER.into(),
            TOKEN.into(),
            readiness_command(&root),
            Some(&dsh_context(&root)),
        );
        assert!(response.ok, "{response:?}");
        assert_eq!(response.data["receive"]["owner"], "gateway");
        assert_eq!(response.data["transport"]["kind"], "dsh");
        assert_eq!(response.data["status"], "ready");

        assert_eq!(std::fs::read(&state_path).unwrap(), state_before);
        assert_eq!(
            std::fs::read(host.join("identity-map.json")).unwrap(),
            map_before
        );
        assert!(!host.join("server.sock").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn capability_readiness_rejects_send_admission_before_mutation() {
        let (server, root, state_path, host) = test_server();
        seed_ready_server(&server, &root);
        let state_before = std::fs::read(&state_path).unwrap();
        let map_before = std::fs::read(host.join("identity-map.json")).unwrap();

        let mut command = readiness_command(&root);
        command.actor_binding_id = BindingId::new("readiness-other-binding").unwrap();

        let response = handle_capability_readiness(
            &server,
            WORKER.into(),
            TOKEN.into(),
            command,
            Some(&dsh_context(&root)),
        );
        assert!(!response.ok, "{response:?}");
        assert!(
            response
                .error
                .as_deref()
                .unwrap()
                .contains("SEND_BINDING_REJECTED"),
            "{response:?}"
        );
        assert_eq!(std::fs::read(&state_path).unwrap(), state_before);
        assert_eq!(
            std::fs::read(host.join("identity-map.json")).unwrap(),
            map_before
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn capability_readiness_rejects_missing_project_context_without_mutation() {
        let (server, root, state_path, _host) = test_server();
        seed_ready_server(&server, &root);
        let state_before = std::fs::read(&state_path).unwrap();

        let response = handle_capability_readiness(
            &server,
            WORKER.into(),
            TOKEN.into(),
            readiness_command(&root),
            None,
        );
        assert!(!response.ok, "{response:?}");
        assert!(
            response
                .error
                .as_deref()
                .unwrap()
                .contains("PROJECT_CONTEXT_REQUIRED"),
            "{response:?}"
        );
        assert_eq!(std::fs::read(&state_path).unwrap(), state_before);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn capability_readiness_rejects_wrong_token_at_the_existing_authentication_boundary() {
        let (server, root, state_path, host) = test_server();
        seed_ready_server(&server, &root);
        let state_before = std::fs::read(&state_path).unwrap();
        let map_before = std::fs::read(host.join("identity-map.json")).unwrap();

        let response = handle_capability_readiness(
            &server,
            WORKER.into(),
            "wrong-token".into(),
            readiness_command(&root),
            Some(&dsh_context(&root)),
        );
        assert!(!response.ok, "{response:?}");
        assert!(
            response
                .error
                .as_deref()
                .unwrap()
                .contains("token mismatch"),
            "{response:?}"
        );
        assert_eq!(std::fs::read(&state_path).unwrap(), state_before);
        assert_eq!(
            std::fs::read(host.join("identity-map.json")).unwrap(),
            map_before
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
