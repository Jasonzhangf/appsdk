#[test]
fn explicit_send_reports_appserver_wake_rejection_after_durable_commit() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register_appserver_peer(
        &server,
        "recipient",
        &test_appserver_transport("thread-recipient-appserver"),
    );
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
