#[test]
fn explicit_send_reports_appserver_wake_rejection_after_durable_commit() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    {
        let mut state = server.state.lock().unwrap();
        let transport = test_appserver_transport("thread-recipient-appserver");
        let thread = transport.thread_id.clone().unwrap();
        state.workers.get_mut("recipient").unwrap().transport = Some(transport);
        let subscription = state
            .notification_subscriptions
            .get_mut("sub-default-direct-message-recipient")
            .unwrap();
        subscription.method = "appserver".into();
        subscription.target = thread;
    }
    server.appserver_notification_sink = Arc::new(|_, _, _, _, _, _| {
        Err("ADAPTER_ROUTE_UNAVAILABLE: turn/start forced failure".into())
    });

    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some(
            "APPSERVER_NOTIFICATION_REJECTED: ADAPTER_ROUTE_UNAVAILABLE: turn/start forced failure"
        )
    );
    assert_eq!(response.data["durable"], true);
    assert_eq!(response.data["notification"], "subscribed-not-sent");
    std::fs::remove_dir_all(root).ok();
}
