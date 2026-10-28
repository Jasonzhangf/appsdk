use super::*;
use crate::server::name_state::{
    AgentActivity as Activity, AgentActivityRecord, AgentPresence as Presence,
    AgentPresenceRecord,
};
use crate::server::state::Event;

fn seed_idle_master(server: &Server, idle_since_ms: i64, observed_ms: i64) -> String {
    let transport = crate::server::peer_tests::test_appserver_transport("thread-master");
    let app = crate::identity::AppServerId::new("appserver-cli").unwrap();
    let cwd = server.root.display().to_string();
    let registered = crate::server::peer_tests::seed_committed_owner(
        server,
        "master",
        "token-master",
        &transport,
        &cwd,
        Some(app.clone()),
    );
    assert!(registered.ok, "master registration failed: {registered:?}");
    crate::server::commit_current_thread_route_for_runtime(
        server,
        server,
        "master",
        &cwd,
        Some(&app),
    )
    .expect("master native thread route must commit");
    crate::server::restore_registered_peer_default_leases(server);
    let promoted = crate::server::handle_master_promote(
        server,
        "master".into(),
        "token-master".into(),
        "user-approved".into(),
    );
    assert!(promoted.ok, "master promotion failed: {promoted:?}");
    let session = server
        .state
        .lock()
        .unwrap()
        .workers
        .get("master")
        .and_then(|worker| worker.transport.as_ref())
        .and_then(|transport| transport.session_id.clone())
        .expect("master transport session");
    let scope = GlobalState::canonical_project_scope(&server.root)
        .expect("canonical project scope")
        .as_str()
        .to_owned();
    server.commit(&[
        Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: scope.clone(),
                worker_id: "master".into(),
                session_id: session.clone(),
                presence: Presence::Online,
                reason: "test idle master".into(),
                observed_ms,
            },
        },
        Event::AgentActivityObserved {
            activity: AgentActivityRecord {
                project_scope: scope.clone(),
                worker_id: "master".into(),
                session_id: session,
                activity: Activity::Idle,
                background: false,
                reason: "test idle master".into(),
                observed_ms,
                idle_since_ms: Some(idle_since_ms),
            },
        },
    ]);
    scope
}

#[test]
fn workspace_idle_view_reports_typed_idle_and_unknown() {
    let (server, root) = crate::server::peer_tests::test_server();
    let now = 2_000_000;
    let scope = seed_idle_master(&server, 1_800_000, now);
    let state = server.state.lock().unwrap();
    let idle = workspace_idle_view(&server, &state, Some("master"), now);
    assert_eq!(idle["project_root"], scope);
    assert_eq!(idle["activity"], "idle");
    assert_eq!(idle["idle_since_ms"], 1_800_000);
    assert_eq!(idle["idle_duration_ms"], 200_000);
    assert_eq!(idle["observed_ms"], now);
    let unknown = workspace_idle_view(&server, &state, None, now);
    assert_eq!(unknown["project_root"], scope);
    assert_eq!(unknown["activity"], "unknown");
    assert_eq!(unknown["idle_since_ms"], serde_json::Value::Null);
    assert_eq!(unknown["idle_duration_ms"], 0);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn master_status_workspace_uses_registered_canonical_project() {
    let (server, root) = crate::server::peer_tests::test_server();
    let now = now_ms();
    let scope = seed_idle_master(&server, now - 60_000, now);
    let response = handle_master_status(&server);
    assert!(response.ok, "master status failed: {response:?}");
    let workspace = &response.data["master"]["workspace"];
    assert_eq!(workspace["project_root"], scope);
    assert_eq!(workspace["activity"], "idle");
    assert_eq!(workspace["idle_since_ms"], now - 60_000);
    assert!(
        workspace["idle_duration_ms"].as_i64().unwrap() >= 60_000,
        "the projection must derive duration from the trusted start: {workspace}"
    );
    assert_eq!(workspace["observed_ms"], now);
    std::fs::remove_dir_all(root).ok();
}
