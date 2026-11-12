#[test]
fn wire_poll_rechecks_current_owner_before_delivering_after_wait() {
    use std::future::Future;
    use std::task::{Context, Wake, Waker};

    struct PollWake(tokio::sync::Notify);
    impl Wake for PollWake {
        fn wake(self: Arc<Self>) { self.0.notify_one(); }
        fn wake_by_ref(self: &Arc<Self>) { self.0.notify_one(); }
    }

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all()
        .max_blocking_threads(1).build().unwrap();
    runtime.block_on(async {
    let (server, root, _) = test_server();
    let app = crate::identity::CLI_APP_SERVER_ID;
    let worker_id = "poll-current-worker";
    let token = "token-poll-current-worker";
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    commit_host_route_fixture(&manager, &server, worker_id, token, app,
        committed_tmux_endpoint(&root, "thread-poll-current"));
    let runtime = runtime_for_registered(&server, &root, worker_id, app);
    let mut poll = Box::pin(dispatch_wire(server.clone(),
        Some(context_with_runtime(&root, app, &runtime)),
        Req::Poll { worker_id: worker_id.into(), token: token.into(), timeout_ms: 10_000,
            receive_id: Some("receive-poll-current".into()) }));
    let wake = Arc::new(PollWake(tokio::sync::Notify::new()));
    let waker = Waker::from(wake.clone());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let blocker = tokio::task::spawn_blocking(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    started_rx.await.unwrap();
    // First poll admits the authenticated request and starts its blocking
    // empty-inbox query. Its JoinHandle wake proves that query has returned.
    assert!(poll.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    release_tx.send(()).unwrap();
    blocker.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), wake.0.notified()).await.unwrap();
    // The completed empty query now parks on mailbox notification. Changing
    // the token after this point tests revalidation after waiting.
    assert!(poll.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    let mut replacement = server.state.lock().unwrap().workers[worker_id].clone();
    replacement.token = "token-current-replacement".into();
    server.commit_checked(&[Event::Registered { worker: replacement }, Event::Sent {
        msg: Message {
            id: "poll-owner-change-message".into(), from: "sender".into(), to: worker_id.into(),
            mtype: "notify".into(), subject: Some("current owner".into()),
            body: "must remain pending".into(), in_reply_to: None, created_ms: now_ms(),
            state: "pending".into(), wake_attempt_count: 0, last_wake_attempt_ms: 0, retry_attempted: false,
        },
    }]).unwrap();
    server.mailbox_notify.notify_waiters();
    let rejected = tokio::time::timeout(Duration::from_secs(5), poll).await.unwrap();
    assert!(!rejected.ok, "{rejected:?}");
    assert!(rejected.error.as_deref().unwrap().contains("token mismatch"), "{rejected:?}");
    let state = server.state.lock().unwrap();
    let message = &state.msgs["poll-owner-change-message"];
    assert_eq!(message.state, "pending");
    assert_eq!(message.body, "must remain pending");
    assert_eq!(message.wake_attempt_count, 0);
    assert!(!state.receive_receipts.contains_key("receive-poll-current"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn wire_poll_rechecks_current_map_before_delivering_after_wait() {
    use std::future::Future;
    use std::task::{Context, Wake, Waker};

    struct PollWake(tokio::sync::Notify);
    impl Wake for PollWake {
        fn wake(self: Arc<Self>) {
            self.0.notify_one();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.notify_one();
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (server, root, _) = test_server();
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let app = crate::identity::CLI_APP_SERVER_ID;
        let old_worker = "poll-map-old";
        let old_token = "token-poll-map-old";
        let new_worker = "poll-map-new";
        let new_token = "token-poll-map-new";
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        commit_host_route_fixture(
            &manager,
            &server,
            old_worker,
            old_token,
            app,
            committed_tmux_endpoint(&root, "thread-poll-map-old"),
        );
        commit_host_route_fixture(
            &manager,
            &server,
            new_worker,
            new_token,
            app,
            committed_tmux_endpoint(&root, "thread-poll-map-new"),
        );
        let old_runtime = runtime_for_registered(&server, &root, old_worker, app);
        let new_runtime = runtime_for_registered(&server, &root, new_worker, app);
        let scope = GlobalState::canonical_project_scope(&root).unwrap();
        let app_scope = AppServerId::new(app).unwrap();
        let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
        let control = store.load().unwrap();
        let old_key = control
            .key_for_worker(&scope, &app_scope, old_worker)
            .unwrap()
            .unwrap()
            .clone();
        let old_name = old_key.name.clone();
        let new_key = control
            .key_for_worker(&scope, &app_scope, new_worker)
            .unwrap()
            .unwrap()
            .clone();
        let new_value = control
            .current_for_worker(&scope, &app_scope, new_worker)
            .unwrap()
            .unwrap()
            .active_binding()
            .unwrap()
            .clone();

        let mut poll = Box::pin(dispatch_wire_routed(
            manager.clone(),
            Some(context_with_runtime(&root, app, &old_runtime)),
            Req::Poll {
                worker_id: old_worker.into(),
                token: old_token.into(),
                timeout_ms: 1_000,
                receive_id: Some("receive-poll-map-current".into()),
            },
            shutdown_rx.clone(),
        ));
        let wake = Arc::new(PollWake(tokio::sync::Notify::new()));
        let waker = Waker::from(wake.clone());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.await.unwrap();
        assert!(poll
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());
        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), wake.0.notified())
            .await
            .unwrap();
        assert!(poll
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());

        let message_id = "poll-map-current-message";
        server
            .commit_checked(&[
                Event::Sent {
                    msg: Message {
                        id: message_id.into(),
                        from: "sender".into(),
                        to: old_worker.into(),
                        mtype: "notify".into(),
                        subject: Some("current map poll".into()),
                        body: "must wait for the current map holder".into(),
                        in_reply_to: None,
                        created_ms: now_ms(),
                        state: "pending".into(),
                        wake_attempt_count: 0,
                        last_wake_attempt_ms: 0,
                        retry_attempted: false,
                    },
                },
                Event::MailboxNameAddressed {
                    message_id: message_id.into(),
                    address: name_mailbox::MailboxNameAddress {
                        project_scope: scope.as_str().into(),
                        name: old_name.clone(),
                    },
                },
                Event::TargetSessionBound {
                    message_id: message_id.into(),
                    target_session: "session-poll-map-hidden".into(),
                },
            ])
            .unwrap();

        store
            .commit(binding_store::ControlChange::MoveName {
                target: old_key,
                value: new_value,
                source_names: vec![new_key.name],
                expected_generation: None,
            })
            .unwrap();
        server
            .commit_checked(&[Event::TargetSessionBound {
                message_id: message_id.into(),
                target_session: old_runtime.session_id.as_ref().unwrap().to_string(),
            }])
            .unwrap();
        server.mailbox_notify.notify_waiters();

        let response = tokio::time::timeout(Duration::from_secs(3), poll)
            .await
            .unwrap()
            .1;
        assert!(response.ok, "{response:?}");
        assert_eq!(response.data["count"], 0, "{response:?}");
        assert_eq!(response.data["timeout"], true, "{response:?}");
        {
            let state = server.state.lock().unwrap();
            assert_eq!(state.msgs[message_id].state, "pending");
            assert!(!state.receive_receipts.contains_key("receive-poll-map-current"));
        }

        let previous = name_state::NameOccupancy {
            name: old_name.clone(),
            worker_id: old_worker.into(),
            session_id: old_runtime.session_id.as_ref().unwrap().to_string(),
            occupancy_revision: 1,
            updated_ms: 1,
            pane_anchor: None,
        };
        let next = name_state::NameOccupancy {
            name: old_name,
            worker_id: new_worker.into(),
            session_id: new_runtime.session_id.as_ref().unwrap().to_string(),
            occupancy_revision: 2,
            updated_ms: 2,
            pane_anchor: None,
        };
        let mut handoff = vec![Event::NameResolutionCommitted {
            project_scope: scope.as_str().into(),
            occupancy: next.clone(),
            assignments: Vec::new(),
        }];
        handoff.extend({
            let state = server.state.lock().unwrap();
            name_mailbox::handoff_events(
                &state,
                scope.as_str(),
                &next,
                &[],
                Some(&previous),
            )
        });
        server.commit_checked(&handoff).unwrap();

        let received = dispatch_wire_routed(
            manager,
            Some(context_with_runtime(&root, app, &new_runtime)),
            Req::Poll {
                worker_id: new_worker.into(),
                token: new_token.into(),
                timeout_ms: 0,
                receive_id: None,
            },
            shutdown_rx,
        )
        .await
        .1;
        assert!(received.ok, "{received:?}");
        assert_eq!(received.data["count"], 1, "{received:?}");
        assert_eq!(received.data["messages"][0]["id"], message_id);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn routed_ack_rechecks_current_map_for_explicit_and_bulk_consumption() {
    let (server, root, _) = test_server();
    let app = crate::identity::CLI_APP_SERVER_ID;
    let old_worker = "ack-map-old";
    let old_token = "token-ack-map-old";
    let new_worker = "ack-map-new";
    let new_token = "token-ack-map-new";
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    commit_host_route_fixture(
        &manager,
        &server,
        old_worker,
        old_token,
        app,
        committed_tmux_endpoint(&root, "thread-ack-map-old"),
    );
    commit_host_route_fixture(
        &manager,
        &server,
        new_worker,
        new_token,
        app,
        committed_tmux_endpoint(&root, "thread-ack-map-new"),
    );
    let old_runtime = runtime_for_registered(&server, &root, old_worker, app);
    let new_runtime = runtime_for_registered(&server, &root, new_worker, app);
    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new(app).unwrap();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let old_key = control
        .key_for_worker(&scope, &app_scope, old_worker)
        .unwrap()
        .unwrap()
        .clone();
    let old_name = old_key.name.clone();
    let new_key = control
        .key_for_worker(&scope, &app_scope, new_worker)
        .unwrap()
        .unwrap()
        .clone();
    let new_value = control
        .current_for_worker(&scope, &app_scope, new_worker)
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    let typed_id = "ack-map-typed";
    let raw_id = "ack-map-raw";
    for (id, addressed_name) in [(typed_id, Some(old_name.clone())), (raw_id, None)] {
        let mut events = vec![Event::Sent {
            msg: Message {
                id: id.into(),
                from: "sender".into(),
                to: old_worker.into(),
                mtype: "notify".into(),
                subject: Some(id.into()),
                body: "durable body".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        }];
        if let Some(name) = addressed_name {
            events.push(Event::MailboxNameAddressed {
                message_id: id.into(),
                address: name_mailbox::MailboxNameAddress {
                    project_scope: scope.as_str().into(),
                    name,
                },
            });
        }
        server.commit_checked(&events).unwrap();
    }

    store
        .commit(binding_store::ControlChange::MoveName {
            target: old_key,
            value: new_value,
            source_names: vec![new_key.name],
            expected_generation: None,
        })
        .unwrap();

    let (_, explicit) = manager.dispatch_sync(
        Some(context_with_runtime(&root, app, &old_runtime)),
        Req::Ack {
            worker_id: old_worker.into(),
            token: old_token.into(),
            ids: vec![typed_id.into()],
        },
    );
    assert!(explicit.ok, "{explicit:?}");
    assert_eq!(explicit.data["not_found"], json!([typed_id]));
    assert_eq!(explicit.data["acked"], json!([]));

    let (_, bulk) = manager.dispatch_sync(
        Some(context_with_runtime(&root, app, &old_runtime)),
        Req::Ack {
            worker_id: old_worker.into(),
            token: old_token.into(),
            ids: Vec::new(),
        },
    );
    assert!(bulk.ok, "{bulk:?}");
    let acked = bulk.data["acked"].as_array().unwrap();
    assert!(acked.iter().any(|id| id == raw_id), "{bulk:?}");
    assert!(!acked.iter().any(|id| id == typed_id), "{bulk:?}");
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[typed_id].state, "pending");
        assert_eq!(state.msgs[raw_id].state, "read");
    }

    let previous = name_state::NameOccupancy {
        name: old_name.clone(),
        worker_id: old_worker.into(),
        session_id: old_runtime.session_id.as_ref().unwrap().to_string(),
        occupancy_revision: 1,
        updated_ms: 1,
        pane_anchor: None,
    };
    let next = name_state::NameOccupancy {
        name: old_name,
        worker_id: new_worker.into(),
        session_id: new_runtime.session_id.as_ref().unwrap().to_string(),
        occupancy_revision: 2,
        updated_ms: 2,
        pane_anchor: None,
    };
    let mut handoff = vec![Event::NameResolutionCommitted {
        project_scope: scope.as_str().into(),
        occupancy: next.clone(),
        assignments: Vec::new(),
    }];
    handoff.extend({
        let state = server.state.lock().unwrap();
        name_mailbox::handoff_events(
            &state,
            scope.as_str(),
            &next,
            &[],
            Some(&previous),
        )
    });
    server.commit_checked(&handoff).unwrap();

    let (_, received) = manager.dispatch_sync(
        Some(context_with_runtime(&root, app, &new_runtime)),
        Req::Inbox {
            worker_id: new_worker.into(),
            token: new_token.into(),
        },
    );
    assert!(received.ok, "{received:?}");
    assert_eq!(received.data["unread"], 1, "{received:?}");
    assert_eq!(received.data["messages"][0]["id"], typed_id);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn waiting_poll_rechecks_same_principal_exact_binding_after_map_refresh() {
    use std::future::Future;
    use std::task::{Context, Wake, Waker};

    struct PollWake(tokio::sync::Notify);
    impl Wake for PollWake {
        fn wake(self: Arc<Self>) {
            self.0.notify_one();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.notify_one();
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(2)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (server, root, _) = test_server();
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "poll-exact-old";
        let token = "token-poll-exact-old";
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        commit_host_route_fixture(
            &manager,
            &server,
            worker_id,
            token,
            app,
            committed_tmux_endpoint(&root, "thread-poll-exact-old"),
        );
        let old_runtime = runtime_for_registered(&server, &root, worker_id, app);
        let scope = GlobalState::canonical_project_scope(&root).unwrap();
        let app_scope = AppServerId::new(app).unwrap();
        let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
        let control = store.load().unwrap();
        let key = control
            .key_for_worker(&scope, &app_scope, worker_id)
            .unwrap()
            .unwrap()
            .clone();
        let name = key.name.clone();
        let old_value = control
            .current_for_worker(&scope, &app_scope, worker_id)
            .unwrap()
            .unwrap()
            .active_binding()
            .unwrap()
            .clone();
        let mut refreshed = old_value;
        refreshed.binding.session_id = Some(
            crate::identity::SessionId::new("session-poll-exact-refreshed").unwrap(),
        );

        let mut poll = Box::pin(dispatch_wire_routed(
            manager,
            Some(context_with_runtime(&root, app, &old_runtime)),
            Req::Poll {
                worker_id: worker_id.into(),
                token: token.into(),
                timeout_ms: 1_000,
                receive_id: Some("receive-poll-exact-refreshed".into()),
            },
            shutdown_rx,
        ));
        let wake = Arc::new(PollWake(tokio::sync::Notify::new()));
        let waker = Waker::from(wake.clone());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.await.unwrap();
        assert!(poll
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());
        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), wake.0.notified())
            .await
            .unwrap();
        assert!(poll
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());

        let message_id = "poll-exact-binding-message";
        server
            .commit_checked(&[
                Event::Sent {
                    msg: Message {
                        id: message_id.into(),
                        from: "sender".into(),
                        to: worker_id.into(),
                        mtype: "notify".into(),
                        subject: Some("exact binding poll".into()),
                        body: "must wait for the refreshed binding".into(),
                        in_reply_to: None,
                        created_ms: now_ms(),
                        state: "pending".into(),
                        wake_attempt_count: 0,
                        last_wake_attempt_ms: 0,
                        retry_attempted: false,
                    },
                },
                Event::MailboxNameAddressed {
                    message_id: message_id.into(),
                    address: name_mailbox::MailboxNameAddress {
                        project_scope: scope.as_str().into(),
                        name,
                    },
                },
                Event::TargetSessionBound {
                    message_id: message_id.into(),
                    target_session: old_runtime.session_id.as_ref().unwrap().to_string(),
                },
            ])
            .unwrap();

        store
            .commit(binding_store::ControlChange::Commit {
                key,
                value: refreshed,
                expected_generation: None,
            })
            .unwrap();
        server.mailbox_notify.notify_waiters();

        let response = tokio::time::timeout(Duration::from_secs(3), poll)
            .await
            .unwrap()
            .1;
        assert!(response.ok, "{response:?}");
        assert_eq!(response.data["count"], 0, "{response:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[message_id].state, "pending");
        assert!(!state
            .receive_receipts
            .contains_key("receive-poll-exact-refreshed"));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn routed_ack_rejects_same_principal_old_binding_after_map_refresh() {
    let (server, root, _) = test_server();
    let app = crate::identity::CLI_APP_SERVER_ID;
    let worker_id = "ack-exact-old";
    let token = "token-ack-exact-old";
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    commit_host_route_fixture(
        &manager,
        &server,
        worker_id,
        token,
        app,
        committed_tmux_endpoint(&root, "thread-ack-exact-old"),
    );
    let old_runtime = runtime_for_registered(&server, &root, worker_id, app);
    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new(app).unwrap();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let key = control
        .key_for_worker(&scope, &app_scope, worker_id)
        .unwrap()
        .unwrap()
        .clone();
    let name = key.name.clone();
    let old_value = control
        .current_for_worker(&scope, &app_scope, worker_id)
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    let mut refreshed = old_value;
    refreshed.binding.session_id = Some(
        crate::identity::SessionId::new("session-ack-exact-refreshed").unwrap(),
    );

    let typed_id = "ack-exact-typed";
    let raw_id = "ack-exact-raw";
    for (id, addressed_name) in [(typed_id, Some(name)), (raw_id, None)] {
        let mut events = vec![Event::Sent {
            msg: Message {
                id: id.into(),
                from: "sender".into(),
                to: worker_id.into(),
                mtype: "notify".into(),
                subject: Some(id.into()),
                body: "durable body".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        }];
        if let Some(name) = addressed_name {
            events.push(Event::MailboxNameAddressed {
                message_id: id.into(),
                address: name_mailbox::MailboxNameAddress {
                    project_scope: scope.as_str().into(),
                    name,
                },
            });
        }
        server.commit_checked(&events).unwrap();
    }

    store
        .commit(binding_store::ControlChange::Commit {
            key,
            value: refreshed,
            expected_generation: None,
        })
        .unwrap();

    let (_, explicit) = manager.dispatch_sync(
        Some(context_with_runtime(&root, app, &old_runtime)),
        Req::Ack {
            worker_id: worker_id.into(),
            token: token.into(),
            ids: vec![typed_id.into()],
        },
    );
    assert!(explicit.ok, "{explicit:?}");
    assert_eq!(explicit.data["not_found"], json!([typed_id]));
    assert_eq!(explicit.data["acked"], json!([]));

    let (_, bulk) = manager.dispatch_sync(
        Some(context_with_runtime(&root, app, &old_runtime)),
        Req::Ack {
            worker_id: worker_id.into(),
            token: token.into(),
            ids: Vec::new(),
        },
    );
    assert!(bulk.ok, "{bulk:?}");
    let acked = bulk.data["acked"].as_array().unwrap();
    assert!(acked.iter().any(|id| id == raw_id), "{bulk:?}");
    assert!(!acked.iter().any(|id| id == typed_id), "{bulk:?}");
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[typed_id].state, "pending");
    assert_eq!(state.msgs[raw_id].state, "read");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
