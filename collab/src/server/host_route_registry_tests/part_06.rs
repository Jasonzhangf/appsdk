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
