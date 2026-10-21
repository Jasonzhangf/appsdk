use super::*;

#[test]
fn typed_presence_read_under_state_lock_does_not_relock() {
    let (server, root) = peer_tests::test_server();
    assert!(peer_tests::register_synthetic_peer(&server, "reader", "reader").ok);
    let server = Arc::new(server);
    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    {
        let mut state = server.state.lock().unwrap();
        let worker = state.workers.get_mut("reader").unwrap();
        worker.transport.as_mut().unwrap().kind = TransportKind::AppServer;
        let session = worker
            .transport
            .as_ref()
            .unwrap()
            .session_id
            .clone()
            .unwrap();
        state.apply(&Event::AgentPresenceObserved {
            presence: name_state::AgentPresenceRecord {
                project_scope: scope.as_str().to_owned(),
                worker_id: "reader".into(),
                session_id: session,
                presence: name_state::AgentPresence::Online,
                reason: "test current observation".into(),
                observed_ms: now_ms(),
            },
        });
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let observed = server.clone();
    let reader = std::thread::spawn(move || {
        let state = observed.state.lock().unwrap();
        let presence = worker_presence_in_state(&observed, &state, &state.workers["reader"]);
        sender.send(presence).unwrap();
    });
    assert_eq!(
        receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        IdentityPresence::Present
    );
    reader.join().unwrap();
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}
