#[tokio::test]
async fn split_journal_same_pane_master_is_fenced_until_host_reconcile() {
    split_journal_context_recovery(true, false).await;
}

#[tokio::test]
async fn staged_split_context_recovery_without_live_pane() {
    split_journal_context_recovery(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_cli_context_recovers_authenticated_staged_split() {
    split_journal_context_recovery(true, true).await;
}

async fn split_journal_context_recovery(native: bool, public_cli: bool) {
    let (host, host_root, _) = test_server();
    let (runtime, project_root, _) = runtime_fixture_for_host(&host);
    let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
    let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
    let worker = "split-pane-master";
    let token = "token-split-pane-master";
    let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
    let old_candidates = if native {
        test_candidates_at("split-pane-old", &project_root).unwrap()
    } else {
        TransportCandidates {
            appserver: None,
            dsh: None,
            tmux: Some(crate::proto::TmuxCandidate {
                endpoint: crate::proto::TmuxEndpoint {
                    socket_path: host_root.join("absent.sock").display().to_string(),
                    server_pid: 42,
                    tmux_session_id: "$7".into(),
                    pane_id: "%3".into(),
                    pane_pid: 99,
                    codex_session_id: Some("session-split-pane-old".into()),
                    codex_thread_id: Some("split-pane-old".into()),
                },
                cwd: project_root.display().to_string(),
            }),
        }
    };
    // This recovery fixture starts from an already committed typed owner.
    // Initial communication admission is covered by the public pane consumer.
    let fixture_transport = |candidates: &TransportCandidates| {
        let endpoint = candidates.tmux.as_ref().unwrap().endpoint.clone();
        SelectedTransport {
            kind: TransportKind::Tmux,
            endpoint: Some(endpoint.socket_path.clone()),
            namespace: Some(endpoint.tmux_session_id.clone()),
            session_id: endpoint.codex_session_id.clone(),
            thread_id: endpoint.codex_thread_id.clone(),
            tmux_endpoint: Some(endpoint),
            capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
            self_check: "previously registered typed-owner fixture".into(),
        }
    };
    let old_transport = fixture_transport(&old_candidates);
    let prepared = prepare_register_typed(
        &runtime,
        worker,
        token,
        &old_transport,
        &project_root.display().to_string(),
        Some(GlobalState::canonical_project_scope(&project_root).unwrap()),
        Some(app.clone()),
        false,
    )
    .unwrap();
    let first = consume_prepared_register_typed(&runtime, worker, &old_transport, prepared, None);
    assert!(first.ok, "{first:?}");
    restore_registered_peer_default_leases(&runtime);
    let old_transport = runtime.state.lock().unwrap().workers[worker]
        .transport
        .clone()
        .unwrap();
    let promoted = super::handle_master_promote(
        &runtime,
        worker.into(),
        token.into(),
        "user approved split-pane-master".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    let old = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &crate::server::global_state::RouteScope {
                app_scope_id: app.clone(),
                project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
            },
            &BindingId::new("binding-split-pane-master").unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: old.clone(),
    }])
    .unwrap();
    manager.install_runtime(
        &(
            app.as_str().to_owned(),
            old.project_scope.as_str().to_owned(),
        ),
        runtime.clone(),
        None,
    );
    let mut new_candidates = old_candidates;
    let endpoint = &mut new_candidates.tmux.as_mut().unwrap().endpoint;
    endpoint.codex_session_id = Some("session-split-pane-new".into());
    endpoint.codex_thread_id = Some("split-pane-new".into());
    let context = context_with_app(&project_root, crate::identity::CLI_APP_SERVER_ID);
    let previous = manager
        .registration_rollback_state(&runtime, &context, worker)
        .unwrap();
    let new_transport = fixture_transport(&new_candidates);
    let prepared = prepare_register_typed(
        &runtime,
        worker,
        token,
        &new_transport,
        &project_root.display().to_string(),
        Some(GlobalState::canonical_project_scope(&project_root).unwrap()),
        Some(app.clone()),
        false,
    )
    .unwrap();
    let second = consume_prepared_register_typed(&runtime, worker, &new_transport, prepared, None);
    assert!(second.ok, "{second:?}");
    restore_registered_peer_default_leases(&runtime);
    manager
        .fail_current_thread_route_publish
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = manager.commit_current_thread_route(
        &runtime,
        &context,
        worker,
        previous.0.as_ref(),
        previous.1.as_ref(),
        previous.2.as_ref(),
        &previous.3,
    );
    assert!(failed.is_err());
    assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
    assert_eq!(
        runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&old.route_scope(), &old.binding_id)
            .unwrap(),
        &old,
    );
    manager
        .fail_current_thread_route_publish
        .store(false, std::sync::atomic::Ordering::SeqCst);
    // Re-establish the staged typed-owner precondition. A public Register here
    // would perform initial candidate admission, which this restoration case
    // expressly excludes (including its absent/killed-pane variants).
    let retry = seed_committed_registration_fixture(
        &runtime,
        worker.into(),
        token.into(),
        project_root.display().to_string(),
        Some(app),
        Some(new_candidates.clone()),
    );
    assert!(retry.ok, "{retry:?}");
    restore_registered_peer_default_leases(&runtime);
    assert_eq!(
        retry.data["command"]["binding"]["endpoint_generation"],
        old.endpoint_generation + 1
    );
    assert!(manager.same_pane_master_route_ready(&runtime).is_err());
    let staged = runtime.state.lock().unwrap().global
        .lookup_binding_for(&old.route_scope(), &old.binding_id).unwrap().clone();
    let committed_ids = (
        retry.data["command_id"].as_str().unwrap().to_owned(),
        retry.data["operation_id"].as_str().unwrap().to_owned(),
    );
    let staged_worker = runtime.state.lock().unwrap().workers[worker].clone();
    let staged_subscriptions = json!(runtime.state.lock().unwrap().notification_subscriptions);
    let pane = old.tmux_endpoint.as_ref().unwrap();
    // Loss of the live pane must not block durable staged/retry ownership.
    if native && !public_cli {
        let output = std::process::Command::new("tmux")
            .args(["-S", &pane.socket_path, "kill-pane", "-t", &pane.pane_id])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(manager.resolve_route_by_tmux_endpoint(pane).is_err());
    assert!(manager
        .resolve_staged_pane_recovery(pane, worker, "wrong-token")
        .is_err());
    let mut wrong_pane = pane.clone();
    wrong_pane.pane_id = "%999999".into();
    assert!(manager
        .resolve_staged_pane_recovery(&wrong_pane, worker, token)
        .is_err());
    let mut wrong_socket = pane.clone();
    wrong_socket.socket_path.push_str(".other");
    assert!(manager
        .resolve_staged_pane_recovery(&wrong_socket, worker, token)
        .is_err());
    assert!(manager
        .resolve_staged_pane_recovery(pane, "other-principal", token)
        .is_err());
    let recovered = manager
        .resolve_staged_pane_recovery(pane, worker, token)
        .unwrap();
    assert_eq!(recovered.endpoint_generation, old.endpoint_generation);
    assert_eq!(recovered.agent_id, old.agent_id);
    assert!(manager.same_pane_master_route_ready(&runtime).is_err());
    let previous_runtime = crate::identity::RuntimeIdentity {
        agent_id: old.agent_id.clone(),
        runtime_id: old.runtime_id.clone(),
        appserver_id: old.app_scope_id.clone(),
        endpoint_generation: old.endpoint_generation,
        binding_id: old.binding_id.clone(),
        session_id: old.session_id.clone(),
        native_thread_id: old.native_thread_id.clone(),
    };
    let mut identity = crate::identity::Identity {
        worker_id: worker.into(),
        token: token.into(),
        project_scope: Some(old.project_scope.clone()),
        runtime: None,
        transport: None,
    };
    crate::identity::persist_registration_at(
        &host_paths,
        &crate::scope::Scope {
            root: project_root.clone(),
        },
        &mut identity,
        previous_runtime,
        old_transport,
    )
    .unwrap();
    if !public_cli {
        let identity_path = host_paths
            .state_root()
            .join("identities")
            .join(worker)
            .join("identity.json");
        let identity_bytes = std::fs::read(&identity_path).unwrap();
        // Current project/session restores the reducer-owned credential even
        // when the local historical receipt has a stale token or generation.
        let mut stale: serde_json::Value = serde_json::from_slice(&identity_bytes).unwrap();
        stale["token"] = json!("stale-local-token");
        stale["runtime"]["endpoint_generation"] = json!(old.endpoint_generation);
        std::fs::write(&identity_path, serde_json::to_vec(&stale).unwrap()).unwrap();
    }
    let grant_before_recovery = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_master_grant_for(&old.route_scope(), &old.binding_id)
        .unwrap()
        .clone();
    if public_cli {
        std::fs::create_dir_all(project_root.join(".agent-collab")).unwrap();
        let listener = tokio::net::UnixListener::bind(host_paths.socket_path()).unwrap();
        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let serving_manager = manager.clone();
        let serving = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(super::conn_task_routed(
                    serving_manager.clone(),
                    stream,
                    receiver.clone(),
                    None,
                ));
            }
        });
        let endpoint = new_candidates.tmux.as_ref().unwrap().endpoint.clone();
        let binary = std::env::var_os("COLLAB_TEST_BINARY")
            .expect("parent must supply freshly built COLLAB_TEST_BINARY for public CLI fixture");
        let state_root = host_paths.state_root().to_path_buf();
        let cli_stdout = host_root.join("public-cli-context.stdout");
        let cli_stderr = host_root.join("public-cli-context.stderr");
        let cli_status = host_root.join("public-cli-context.status");
        let cli_script = host_root.join("public-cli-context.sh");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let script = format!(
            "#!/bin/sh\n\
             unset CODEX_APP_SERVER_SOCKET COLLAB_APPSERVER_SOCKET DSH_SESSION_ID COLLAB_SOCKET_PATH COLLAB_HOST_SOCKET TMUX TMUX_PANE\n\
             export COLLAB_STATE_DIR={state}\n\
             export CODEX_SESSION_ID={session}\n\
             export CODEX_THREAD_ID={thread}\n\
             {binary} context --op ctxop-host-staged-split >{stdout} 2>{stderr}\n\
             status=$?\n\
             printf '%s' \"$status\" >{status_path}\n\
             exec sleep 3600\n",
            state = quote(&state_root.to_string_lossy()),
            session = quote(
                endpoint
                    .codex_session_id
                    .as_deref()
                    .expect("public CLI session id")
            ),
            thread = quote(
                endpoint
                    .codex_thread_id
                    .as_deref()
                    .expect("public CLI thread id")
            ),
            binary = quote(&binary.to_string_lossy()),
            stdout = quote(&cli_stdout.to_string_lossy()),
            stderr = quote(&cli_stderr.to_string_lossy()),
            status_path = quote(&cli_status.to_string_lossy()),
        );
        std::fs::write(&cli_script, script).unwrap();
        let host_journal = host.journal_path.clone();
        let project_journal = runtime.journal_path.clone();
        let mutation_manager = manager.clone();
        let mutation_runtime = runtime.clone();
        let mutation_scope = old.route_scope();
        let mutation_binding_id = old.binding_id.clone();
        let cli_cwd = project_root.clone();
        let mutation_worker = worker.to_string();
        let output = tokio::task::spawn_blocking(move || {
            let run_cli = || {
                for path in [&cli_stdout, &cli_stderr, &cli_status] {
                    let _ = std::fs::remove_file(path);
                }
                let respawn = std::process::Command::new("tmux")
                    .args([
                        "-S",
                        &endpoint.socket_path,
                        "respawn-pane",
                        "-k",
                        "-t",
                        &endpoint.pane_id,
                        "-c",
                        cli_cwd.to_str().expect("UTF-8 project root"),
                        &format!("sh {}", quote(&cli_script.to_string_lossy())),
                    ])
                    .output()
                    .unwrap();
                assert!(
                    respawn.status.success(),
                    "respawn owned public CLI pane: {}",
                    String::from_utf8_lossy(&respawn.stderr)
                );
                for _ in 0..1000 {
                    if cli_status.is_file() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                let status = std::fs::read_to_string(&cli_status)
                    .expect("public CLI in owned pane did not report exit status")
                    .trim()
                    .parse::<i32>()
                    .unwrap();
                (
                    status,
                    std::fs::read(&cli_stdout).unwrap_or_default(),
                    std::fs::read(&cli_stderr).unwrap_or_default(),
                )
            };
            let first = run_cli();
            assert_eq!(first.0, 0, "{}", String::from_utf8_lossy(&first.2));
            let initial: serde_json::Value = serde_json::from_slice(&first.1).unwrap();
            assert_eq!(initial["result"]["snapshot"]["inbox"]["unread"], 0);
            let host_initial = std::fs::read(&host_journal).unwrap();
            let project_initial = std::fs::read(&project_journal).unwrap();
            let unchanged_repeat = run_cli();
            assert_eq!(unchanged_repeat.0, 0, "{}", String::from_utf8_lossy(&unchanged_repeat.2));
            let unchanged: serde_json::Value = serde_json::from_slice(&unchanged_repeat.1).unwrap();
            assert_eq!(unchanged["result"]["outcome"], "completed");
            assert_eq!(unchanged["result"]["snapshot"]["binding"], initial["result"]["snapshot"]["binding"]);
            assert_eq!(unchanged["result"]["snapshot"]["identity"], initial["result"]["snapshot"]["identity"]);
            assert_eq!(unchanged["result"]["snapshot"]["peers"], initial["result"]["snapshot"]["peers"]);
            assert_eq!(unchanged["result"]["snapshot"]["inbox"]["unread"], 0);
            assert_eq!(std::fs::read(&host_journal).unwrap(), host_initial);
            assert_eq!(std::fs::read(&project_journal).unwrap(), project_initial);
            // Mutate through the existing authenticated public message owner.
            // A cached first snapshot cannot satisfy this current-state readback.
            let (mutation_context, mutation_token, mutation_command) = {
                let state = mutation_runtime.state.lock().unwrap();
                let current = state.global
                    .lookup_binding_for(&mutation_scope, &mutation_binding_id)
                    .expect("current binding after CLI recovery")
                    .clone();
                let context = ProjectContext::for_registered_route(
                    &cli_cwd,
                    &crate::identity::RuntimeIdentity {
                        agent_id: current.agent_id.clone(),
                        runtime_id: current.runtime_id.clone(),
                        appserver_id: current.app_scope_id.clone(),
                        endpoint_generation: current.endpoint_generation,
                        binding_id: current.binding_id.clone(),
                        session_id: current.session_id.clone(),
                        native_thread_id: current.native_thread_id.clone(),
                    },
                ).unwrap();
                let token = state.workers[&mutation_worker].token.clone();
                let command = CommandEnvelope::new(
                    CommandId::new("staged-current-state-send-command").unwrap(),
                    OperationId::new("staged-current-state-send-operation").unwrap(),
                    current.binding_id.clone(),
                    current.endpoint_generation,
                    current.route_scope(),
                    None, None, None, None,
                );
                (context, token, command)
            };
            let (_, sent) = mutation_manager.dispatch_sync(Some(mutation_context), Req::Send {
                from: mutation_worker.clone(), worker_id: Some(mutation_worker.clone()),
                token: Some(mutation_token), command: Some(mutation_command), to: mutation_worker,
                mtype: "notify".into(), subject: Some("current-context-readback".into()),
                body: "A message committed after the original context completed.".into(),
                in_reply_to: None, delivery: "queued".into(),
            });
            assert!(sent.ok, "{sent:?}");
            assert_eq!(sent.data["durable"], true);
            let message_id = sent.data["msg_id"].as_str().unwrap();
            let host_before = std::fs::read(&host_journal).unwrap();
            let project_before = std::fs::read(&project_journal).unwrap();
            let repeated = run_cli();
            assert_eq!(repeated.0, 0, "{}", String::from_utf8_lossy(&repeated.2));
            let again: serde_json::Value = serde_json::from_slice(&repeated.1).unwrap();
            assert_eq!(again["result"]["outcome"], "completed");
            assert_eq!(again["result"]["operation_id"], initial["result"]["operation_id"]);
            assert_eq!(again["result"]["phase"], initial["result"]["phase"]);
            assert_eq!(again["result"]["committed_phases"], initial["result"]["committed_phases"]);
            assert_eq!(again["result"]["snapshot"]["binding"], initial["result"]["snapshot"]["binding"]);
            assert_eq!(again["result"]["snapshot"]["identity"], initial["result"]["snapshot"]["identity"]);
            assert_eq!(again["result"]["snapshot"]["peers"], initial["result"]["snapshot"]["peers"]);
            for field in ["worker_id", "scope", "grant_id", "grant_generation", "binding_id", "endpoint_generation"] {
                assert_eq!(again["result"]["snapshot"]["master"][field],
                    initial["result"]["snapshot"]["master"][field]);
            }
            assert_eq!(again["bootstrap"]["identity"], "daemon-owned");
            assert_eq!(again["bootstrap"]["registered"], true);
            assert_eq!(again["result"]["snapshot"]["inbox"]["unread"], 1);
            assert_eq!(again["result"]["snapshot"]["inbox"]["messages"][0]["id"], message_id);
            assert_eq!(std::fs::read(&host_journal).unwrap(), host_before);
            assert_eq!(std::fs::read(&project_journal).unwrap(), project_before);
            first
        })
        .await
        .unwrap();
        shutdown.send(true).unwrap();
        serving.abort();
        assert!(
            output.0 == 0,
            "CLI exit={:?} stdout={} stderr={}",
            output.0,
            String::from_utf8_lossy(&output.1),
            String::from_utf8_lossy(&output.2)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.1).unwrap();
        assert_eq!(value["result"]["outcome"], "completed", "{value}");
        let current = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&old.route_scope(), &old.binding_id)
            .expect("binding after public CLI recovery")
            .clone();
        assert_eq!(
            value["result"]["snapshot"]["binding"]["endpoint_generation"],
            current.endpoint_generation,
            "{value}"
        );
        // The live public CLI rebinds the staged identity once; the durable
        // worker/route owner stays the same while the endpoint advances.
        assert_eq!(current.endpoint_generation, staged.endpoint_generation + 1);
        assert_eq!(current.binding_id, staged.binding_id);
        assert_eq!(current.agent_id, staged.agent_id);
        assert_eq!(current.runtime_id, staged.runtime_id);
        assert_eq!(current.app_scope_id, staged.app_scope_id);
        assert_eq!(current.project_scope, staged.project_scope);
        assert!(current.tmux_endpoint.is_some());
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        let current_grant = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_master_grant_for(&old.route_scope(), &old.binding_id)
            .expect("master grant after public CLI recovery")
            .clone();
        // The same grant stays durable. Only its endpoint generation follows
        // the live binding; grant identity and generation do not reset.
        assert_eq!(current_grant.grant_id, grant_before_recovery.grant_id);
        assert_eq!(current_grant.grant_generation, grant_before_recovery.grant_generation);
        assert_eq!(current_grant.binding_id, grant_before_recovery.binding_id);
        assert_eq!(current_grant.agent_id, grant_before_recovery.agent_id);
        assert_eq!(current_grant.endpoint_generation, current.endpoint_generation);
        let project_replay = replay_from_journal(&runtime.root, &runtime.journal_path).unwrap();
        assert_eq!(project_replay.global.lookup_binding_for(&old.route_scope(), &old.binding_id), Some(&current));
        assert_eq!(project_replay.global.lookup_master_grant_for(&old.route_scope(), &old.binding_id), Some(&current_grant));
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
        return;
    }
    let (_, completed) = manager.dispatch_sync(
        Some(context),
        Req::IdentityContext {
            facts: crate::proto::IdentityFacts {
                // The staged proof uses the persisted exact endpoint and
                // credential; this report is not live admission evidence.
                tmux: new_candidates.tmux.clone(),
                ..Default::default()
            },
            identity_context: None,
        },
    );
    assert!(completed.ok, "{completed:?}");
    assert_eq!(
        completed.data["snapshot"]["binding"]["endpoint_generation"],
        old.endpoint_generation + 1
    );
    assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
    assert_eq!(
        runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_master_grant_for(&old.route_scope(), &old.binding_id),
        Some(&grant_before_recovery)
    );
    let binding_before_read = completed.data["snapshot"]["binding"].clone();
    let (_, ordinary) = manager.dispatch_sync(
        Some(context_with_app(
            &project_root,
            crate::identity::CLI_APP_SERVER_ID,
        )),
        Req::IdentityContext {
            facts: crate::proto::IdentityFacts {
                tmux: new_candidates.tmux,
                ..Default::default()
            },
            identity_context: None,
        },
    );
    assert!(
        ordinary.ok,
        "ordinary context after convergence: {ordinary:?}"
    );
    assert_eq!(ordinary.data["snapshot"]["binding"], binding_before_read);
    let host_replay = replay_from_journal(&host.root, &host.journal_path).unwrap();
    let project_replay = replay_from_journal(&runtime.root, &runtime.journal_path).unwrap();
    assert_eq!(host_replay.global.lookup_unique_tmux_pane_route(pane), Some(&staged));
    assert_eq!(project_replay.global.lookup_binding_for(&old.route_scope(), &old.binding_id), Some(&staged));
    assert_eq!(json!(project_replay.workers[worker]), json!(staged_worker));
    assert_eq!(project_replay.global.lookup_master_grant_for(&old.route_scope(), &old.binding_id), Some(&grant_before_recovery));
    assert_eq!(json!(project_replay.notification_subscriptions), staged_subscriptions);
    assert!(project_replay.global.command_receipts.contains_key(&committed_ids.0));
    assert_eq!(project_replay.global.command_receipts[&committed_ids.0].operation_id.as_str(), committed_ids.1);
    std::fs::remove_dir_all(host_root).unwrap();
    std::fs::remove_dir_all(project_root).unwrap();
}

#[tokio::test]
async fn startup_reconciles_a_lagged_same_pane_master_route() {
    let (host, host_root, _) = test_server();
    let (runtime, project_root, _) = test_server();
    let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
    let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
    let worker = "lagged-startup-pane-master";
    let token = "token-lagged-startup-pane-master";
    let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
    let candidates = test_candidates("lagged-startup-pane-master").unwrap();
    let first = seed_committed_registration_fixture(
        &runtime,
        worker.into(),
        token.into(),
        project_root.display().to_string(),
        Some(app.clone()),
        Some(candidates.clone()),
    );
    assert!(first.ok, "{first:?}");
    let promoted = handle_master_promote(
        &runtime,
        worker.into(),
        token.into(),
        "user approved lagged startup pane master".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    let mut binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &crate::server::global_state::RouteScope {
                app_scope_id: app.clone(),
                project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
            },
            &BindingId::new("binding-lagged-startup-pane-master").unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: binding.clone(),
    }])
    .unwrap();
    binding.endpoint_generation += 4;
    manager.install_runtime(
        &(
            app.as_str().to_owned(),
            binding.project_scope.as_str().to_owned(),
        ),
        runtime.clone(),
        None,
    );
    manager.reconcile_same_pane_master_routes().unwrap();
    let endpoint = binding
        .tmux_endpoint
        .as_ref()
        .expect("same-pane binding has a pane route");
    let reconciled = host
        .state
        .lock()
        .unwrap()
        .global
        .lookup_unique_tmux_pane_route(endpoint)
        .unwrap()
        .clone();
    assert_eq!(
        reconciled.endpoint_generation,
        binding.endpoint_generation - 4
    );
    std::fs::remove_dir_all(host_root).unwrap();
    std::fs::remove_dir_all(project_root).unwrap();
}

#[tokio::test]
async fn superseded_same_pane_master_does_not_fence_project_route() {
    let (host, host_root, _) = test_server();
    let (runtime, project_root, _) = test_server();
    let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
    let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
    let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

    // Worker A is the stale same-pane master.
    let master_worker = "stale-same-pane-master";
    let master_token = "token-stale-same-pane-master";
    let master_candidates = test_candidates(master_worker).unwrap();
    let first = seed_committed_registration_fixture(
        &runtime,
        master_worker.into(),
        master_token.into(),
        project_root.display().to_string(),
        Some(app.clone()),
        Some(master_candidates.clone()),
    );
    assert!(first.ok, "{first:?}");
    let promoted = handle_master_promote(
        &runtime,
        master_worker.into(),
        master_token.into(),
        "user approved same-pane master".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    let scope = crate::server::global_state::RouteScope {
        app_scope_id: app.clone(),
        project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
    };
    let master_binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &scope,
            &BindingId::new(format!("binding-{master_worker}")).unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: master_binding.clone(),
    }])
    .unwrap();
    manager.install_runtime(
        &(
            app.as_str().to_owned(),
            master_binding.project_scope.as_str().to_owned(),
        ),
        runtime.clone(),
        None,
    );
    // With only the master route published the fence is satisfied.
    assert!(manager.same_pane_master_route_ready(&runtime).is_ok());

    // Worker B takes over the same tmux pane with a live native thread.
    let peer_worker = "live-same-pane-peer";
    let peer_token = "token-live-same-pane-peer";
    let mut peer_candidates = master_candidates;
    let endpoint = &mut peer_candidates.tmux.as_mut().unwrap().endpoint;
    endpoint.codex_session_id = Some("session-live-same-pane-peer".into());
    endpoint.codex_thread_id = Some("live-same-pane-peer-thread".into());
    let peer = seed_committed_registration_fixture(
        &runtime,
        peer_worker.into(),
        peer_token.into(),
        project_root.display().to_string(),
        Some(app.clone()),
        Some(peer_candidates),
    );
    assert!(peer.ok, "{peer:?}");
    let peer_binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &scope,
            &BindingId::new(format!("binding-{peer_worker}")).unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: peer_binding.clone(),
    }])
    .unwrap();

    // Publishing the peer at a second address evicts the master anchor from
    // the pane: one pane owns exactly one binding host-wide. The superseded
    // master anchor must no longer fence the project route, and the startup
    // reconciler must not resurrect it.
    assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
    manager.reconcile_same_pane_master_routes().unwrap();
    assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
    {
        let state = host.state.lock().unwrap();
        let endpoint = peer_binding.tmux_endpoint.as_ref().unwrap();
        assert_eq!(
            state.global.tmux_pane_route_claimants(endpoint).len(),
            1,
            "the reconciler must not republish a superseded pane claim"
        );
    }
    std::fs::remove_dir_all(host_root).unwrap();
    std::fs::remove_dir_all(project_root).unwrap();
}

/// One pane owns exactly one binding host-wide, across projects.  A second
/// project that takes the pane evicts the first project's claimant, and the
/// evicted claimant must still not fence its own project's route.
#[tokio::test]
async fn cross_scope_pane_claimant_takes_the_pane_without_fencing_the_project_route() {
    let (host, host_root, _) = test_server();
    let (runtime, project_root, _) = test_server();
    let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
    let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
    let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

    let master_worker = "cross-scope-pane-master";
    let master_token = "token-cross-scope-pane-master";
    // This guard compares prior durable host claims across projects. A live
    // initial participant adds no guarantee to the claimant/index predicate.
    let candidates = TransportCandidates {
        appserver: None,
        dsh: None,
        tmux: Some(crate::proto::TmuxCandidate {
            endpoint: committed_tmux_endpoint(&project_root, master_worker),
            cwd: project_root.display().to_string(),
        }),
    };
    let registered = seed_committed_registration_fixture(
        &runtime,
        master_worker.into(),
        master_token.into(),
        project_root.display().to_string(),
        Some(app.clone()),
        Some(candidates.clone()),
    );
    assert!(registered.ok, "{registered:?}");
    let promoted = handle_master_promote(
        &runtime,
        master_worker.into(),
        master_token.into(),
        "user approved cross-scope pane master".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    let scope = crate::server::global_state::RouteScope {
        app_scope_id: app.clone(),
        project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
    };
    let master_binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &scope,
            &BindingId::new(format!("binding-{master_worker}")).unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: master_binding.clone(),
    }])
    .unwrap();
    manager.install_runtime(
        &(
            app.as_str().to_owned(),
            master_binding.project_scope.as_str().to_owned(),
        ),
        runtime.clone(),
        None,
    );

    // A second project registers a peer on the same tmux pane.  Its codex
    // session and thread differ, so it is a separate route address, and its
    // project scope differs, so it is a separate claimant group.
    let (peer_runtime, peer_root, _) = test_server();
    let peer_worker = "cross-scope-pane-peer";
    let mut peer_candidates = candidates;
    let peer_endpoint = &mut peer_candidates.tmux.as_mut().unwrap().endpoint;
    peer_endpoint.codex_session_id = Some("session-cross-scope-pane-peer".into());
    peer_endpoint.codex_thread_id = Some("cross-scope-pane-peer-thread".into());
    let peer = seed_committed_registration_fixture(
        &peer_runtime,
        peer_worker.into(),
        "token-cross-scope-pane-peer".into(),
        peer_root.display().to_string(),
        Some(app.clone()),
        Some(peer_candidates),
    );
    assert!(peer.ok, "{peer:?}");
    let peer_scope = crate::server::global_state::RouteScope {
        app_scope_id: app.clone(),
        project_scope_id: GlobalState::canonical_project_scope(&peer_root).unwrap(),
    };
    let peer_binding = peer_runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &peer_scope,
            &BindingId::new(format!("binding-{peer_worker}")).unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: peer_binding.clone(),
    }])
    .unwrap();

    let endpoint = master_binding
        .tmux_endpoint
        .clone()
        .expect("the master has a pane route");
    {
        let state = host.state.lock().unwrap();
        let claimants = state.global.tmux_pane_route_claimants(&endpoint);
        assert_eq!(
            claimants.len(),
            1,
            "one pane owns exactly one binding host-wide"
        );
        assert_eq!(
            claimants[0].binding_id, peer_binding.binding_id,
            "the later writer takes the pane"
        );
        assert_eq!(
            state
                .global
                .lookup_unique_tmux_pane_route(&endpoint)
                .map(|binding| binding.binding_id.clone()),
            Some(peer_binding.binding_id.clone())
        );
        assert!(
            state
                .global
                .lookup_current_thread_route(
                    master_binding.session_id.as_ref().unwrap(),
                    master_binding.native_thread_id.as_ref().unwrap(),
                )
                .is_none(),
            "the evicted project's address is gone from the index"
        );
    }
    let ready = manager.same_pane_master_route_ready(&runtime);
    assert!(ready.is_ok(), "an evicted claimant must not fence another project's route: {ready:?}");
    assert!(
        manager.reconcile_same_pane_master_routes().is_ok(),
        "the startup reconciler must skip the evicted claimant instead of failing"
    );
    std::fs::remove_dir_all(host_root).unwrap();
    std::fs::remove_dir_all(project_root).unwrap();
    std::fs::remove_dir_all(peer_root).unwrap();
}

#[tokio::test]
async fn same_pane_master_still_fences_when_host_route_is_missing() {
    let (host, host_root, _) = test_server();
    let (runtime, project_root, _) = test_server();
    let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
    let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
    let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

    let master_worker = "missing-host-route-pane-master";
    let master_token = "token-missing-host-route-pane-master";
    let candidates = test_candidates(master_worker).unwrap();
    let first = seed_committed_registration_fixture(
        &runtime,
        master_worker.into(),
        master_token.into(),
        project_root.display().to_string(),
        Some(app.clone()),
        Some(candidates.clone()),
    );
    assert!(first.ok, "{first:?}");
    let promoted = handle_master_promote(
        &runtime,
        master_worker.into(),
        master_token.into(),
        "user approved same-pane master".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    let scope = crate::server::global_state::RouteScope {
        app_scope_id: app.clone(),
        project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
    };
    let master_binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &scope,
            &BindingId::new(format!("binding-{master_worker}")).unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: master_binding.clone(),
    }])
    .unwrap();
    manager.install_runtime(
        &(
            app.as_str().to_owned(),
            master_binding.project_scope.as_str().to_owned(),
        ),
        runtime.clone(),
        None,
    );
    assert!(manager.same_pane_master_route_ready(&runtime).is_ok());

    // The host route is gone and no route holds the pane, so the project
    // route is unowned and the request must fail closed with the remedy.
    host.commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
        binding: master_binding.clone(),
    }])
    .unwrap();
    let fenced = manager
        .same_pane_master_route_ready(&runtime)
        .expect_err("a missing host route must fence the request");
    assert!(fenced.contains("RECOVERY_RECONCILE_REQUIRED"), "{fenced}");
    assert!(fenced.contains("re-register"), "{fenced}");

    // A second address then takes the same pane. The master anchor is
    // superseded by it, so the project route must not fail closed again.
    let mut peer_candidates = candidates.clone();
    let endpoint = &mut peer_candidates.tmux.as_mut().unwrap().endpoint;
    endpoint.codex_session_id = Some("session-active-same-pane-peer".into());
    endpoint.codex_thread_id = Some("active-same-pane-peer-thread".into());
    let peer = seed_committed_registration_fixture(
        &runtime,
        "active-same-pane-peer".into(),
        "token-active-same-pane-peer".into(),
        project_root.display().to_string(),
        Some(app.clone()),
        Some(peer_candidates),
    );
    assert!(peer.ok, "{peer:?}");
    let peer_binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &scope,
            &BindingId::new("binding-active-same-pane-peer".to_string()).unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: peer_binding,
    }])
    .unwrap();
    assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
    std::fs::remove_dir_all(host_root).unwrap();
    std::fs::remove_dir_all(project_root).unwrap();
}

#[tokio::test]
async fn closed_same_pane_peer_keeps_the_pane_ownership() {
    let (host, host_root, _) = test_server();
    let (runtime, project_root, _) = test_server();
    let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
    let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
    let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

    let master_worker = "closed-peer-pane-master";
    let master_token = "token-closed-peer-pane-master";
    let candidates = test_candidates(master_worker).unwrap();
    let first = seed_committed_registration_fixture(
        &runtime,
        master_worker.into(),
        master_token.into(),
        project_root.display().to_string(),
        Some(app.clone()),
        Some(candidates.clone()),
    );
    assert!(first.ok, "{first:?}");
    let promoted = handle_master_promote(
        &runtime,
        master_worker.into(),
        master_token.into(),
        "user approved same-pane master".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    let scope = crate::server::global_state::RouteScope {
        app_scope_id: app.clone(),
        project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
    };
    let master_binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &scope,
            &BindingId::new(format!("binding-{master_worker}")).unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: master_binding.clone(),
    }])
    .unwrap();
    manager.install_runtime(
        &(
            app.as_str().to_owned(),
            master_binding.project_scope.as_str().to_owned(),
        ),
        runtime.clone(),
        None,
    );
    assert!(manager.same_pane_master_route_ready(&runtime).is_ok());

    // A second peer takes over the pane. The recorded master loses the pane
    // and is closed, because a pane has one owner and can never prove
    // liveness.
    let peer_worker = "closed-same-pane-peer";
    let peer_token = "token-closed-same-pane-peer";
    let mut peer_candidates = candidates.clone();
    let endpoint = &mut peer_candidates.tmux.as_mut().unwrap().endpoint;
    endpoint.codex_session_id = Some("session-closed-same-pane-peer".into());
    endpoint.codex_thread_id = Some("closed-same-pane-peer-thread".into());
    let peer = seed_committed_registration_fixture(
        &runtime,
        peer_worker.into(),
        peer_token.into(),
        project_root.display().to_string(),
        Some(app.clone()),
        Some(peer_candidates),
    );
    assert!(peer.ok, "{peer:?}");
    assert!(
        runtime
            .state
            .lock()
            .unwrap()
            .workers
            .get(master_worker)
            .is_none(),
        "the later registrant takes the pane and closes the previous claimant"
    );
    let peer_binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &scope,
            &BindingId::new(format!("binding-{peer_worker}")).unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: peer_binding.clone(),
    }])
    .unwrap();

    // Another master closes the peer. The peer's durable host route survives
    // the close, so the pane stays owned by a worker that is gone.
    let closer_worker = "closed-peer-pane-closer";
    let closer_token = "token-closed-peer-pane-closer";
    let closer = seed_committed_registration_fixture(
        &runtime,
        closer_worker.into(),
        closer_token.into(),
        project_root.display().to_string(),
        Some(app.clone()),
        Some(test_candidates(closer_worker).unwrap()),
    );
    assert!(closer.ok, "{closer:?}");
    let closer_promoted = handle_master_promote(
        &runtime,
        closer_worker.into(),
        closer_token.into(),
        "user approved closer".into(),
    );
    assert!(closer_promoted.ok, "{closer_promoted:?}");
    let closer_binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &scope,
            &BindingId::new(format!("binding-{closer_worker}")).unwrap(),
        )
        .unwrap()
        .clone();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: closer_binding,
    }])
    .unwrap();
    // Closing the peer through the real close path needs the snapshot
    // receipt its guard requires; the fixture supplies it directly.
    let peer_thread_id = runtime
        .state
        .lock()
        .unwrap()
        .workers
        .get(peer_worker)
        .and_then(|worker| worker.transport.as_ref())
        .and_then(|transport| transport.thread_id.clone())
        .unwrap();
    runtime.state.lock().unwrap().worker_snapshots.insert(
        peer_worker.to_string(),
        crate::server::state::WorkerSnapshotReceipt {
            worker_id: peer_worker.to_string(),
            thread_id: peer_thread_id,
            captured_ms: 0,
        },
    );
    let closed = handle_worker_close(
        &runtime,
        closer_worker.into(),
        closer_token.into(),
        peer_worker.into(),
        "peer finished".into(),
    );
    assert!(closed.ok, "{closed:?}");
    assert!(runtime
        .state
        .lock()
        .unwrap()
        .workers
        .get(peer_worker)
        .is_none());
    assert!(
        host.state
            .lock()
            .unwrap()
            .global
            .current_thread_routes
            .values()
            .any(|route| route.binding_id == peer_binding.binding_id),
        "a closed peer keeps its durable host route"
    );

    // The pane is still held by the closed peer's durable route. Ownership
    // is a fact of the index and not of liveness, so the master route check
    // must not fence on it.
    let fenced = manager.same_pane_master_route_ready(&runtime);
    assert!(fenced.is_ok(), "{fenced:?}");
    {
        let state = host.state.lock().unwrap();
        assert_eq!(
            state
                .global
                .tmux_pane_route_claimants(peer_binding.tmux_endpoint.as_ref().unwrap())
                .len(),
            1,
            "the closed peer's route still owns the pane"
        );
    }
    std::fs::remove_dir_all(host_root).unwrap();
    std::fs::remove_dir_all(project_root).unwrap();
}

include!("part_02_tail.rs");
