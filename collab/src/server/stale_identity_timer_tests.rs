use super::*;
use crate::identity::AppServerId;
use crate::server::peer_tests::{seed_committed_owner, test_appserver_transport, test_server};

fn register_missing_appserver(server: &Server, worker_id: &str, thread_id: &str) {
    let app = AppServerId::new("tui-default").unwrap();
    let response = seed_committed_owner(
        server,
        worker_id,
        &format!("token-{worker_id}"),
        &test_appserver_transport(thread_id),
        &server.root.display().to_string(),
        Some(app),
    );
    assert!(response.ok, "seed missing AppServer peer: {response:?}");
}

#[test]
fn tick_runs_stale_maintenance_on_first_tick_then_after_ten_minutes() {
    let (mut server, root) = test_server();
    server.appserver_thread_status = Arc::new(|_, _| Err("thread not found".into()));
    register_missing_appserver(&server, "timer-first", "thread-first");
    let server = Arc::new(server);

    tick_at(&server, 1_000_000);
    assert!(
        !server
            .state
            .lock()
            .unwrap()
            .workers
            .contains_key("timer-first"),
        "the first tick must run maintenance"
    );

    register_missing_appserver(&server, "timer-second", "thread-second");
    tick_at(&server, 1_000_001);
    assert!(
        server
            .state
            .lock()
            .unwrap()
            .workers
            .contains_key("timer-second"),
        "a tick inside the watermark must not probe again"
    );

    tick_at(&server, 1_600_000);
    assert!(
        !server
            .state
            .lock()
            .unwrap()
            .workers
            .contains_key("timer-second"),
        "the maintenance round must run again after ten minutes"
    );

    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}
