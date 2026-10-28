use super::*;
use crate::server::peer_tests::{register_synthetic_peer, test_server};

fn project_context(root: &Path) -> ProjectContext {
    ProjectContext::for_registered_root_with_app(root, AppServerId::new("tui-default").unwrap())
        .unwrap()
}

fn self_close_request(
    server: &Server,
    context: &ProjectContext,
    worker_id: &str,
    operation_id: &str,
) -> PeerLifecycleRequest {
    let target = {
        let state = server.state.lock().unwrap();
        let worker = state
            .workers
            .get(worker_id)
            .unwrap_or_else(|| panic!("registered worker {worker_id}"));
        exact_target(&state, context, worker).unwrap()
    };
    PeerLifecycleRequest::Close {
        worker_id: worker_id.into(),
        token: format!("token-{worker_id}"),
        operation_id: operation_id.into(),
        query_capability: format!("query-{operation_id}"),
        target,
        reason: "self control-plane removal with open business records".into(),
    }
}

#[test]
fn self_close_with_open_task_succeeds_and_preserves_business_records() {
    let (server, root) = test_server();
    let context = project_context(&root);
    let peer = "peer-self";
    let other = "peer-other";
    assert!(register_synthetic_peer(&server, peer, "%self").ok);
    assert!(register_synthetic_peer(&server, other, "%other").ok);
    assert!(
        handle_master_promote(
            &server,
            peer.into(),
            format!("token-{peer}"),
            "user approved current-pane self removal".into(),
        )
        .ok
    );

    let binding = {
        let state = server.state.lock().unwrap();
        state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == peer)
            .expect("self binding")
            .clone()
    };
    let endpoint = binding.tmux_endpoint.clone().expect("tmux endpoint");
    let now = now_ms();
    server.commit(&[
        state::Event::TaskUpdated {
            task: state::TaskRec {
                id: "self-open-task".into(),
                owner: peer.into(),
                created_by: peer.into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "P1".into(),
                status: "working".into(),
                next_step: Some("continue after control-plane removal".into()),
                wait: None,
                created_ms: now,
                updated_ms: now,
            },
        },
        state::Event::NotificationSubscribed {
            subscription: state::NotificationSubscription {
                id: "self-subscription".into(),
                worker_id: peer.into(),
                event: "direct-message".into(),
                subject: None,
                target: endpoint.pane_id.clone(),
                method: "tmux".into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: i64::MAX,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
            },
        },
        state::Event::Sent {
            msg: state::Message {
                id: "self-pending-message".into(),
                from: other.into(),
                to: peer.into(),
                mtype: "notify".into(),
                subject: Some("preserve me".into()),
                body: "business payload".into(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
    ]);

    let request = self_close_request(&server, &context, peer, "self-with-open-work");
    let response = handle(&server, Some(&context), request);
    let result = &response.data["result"];
    assert_eq!(result["outcome"], "complete", "{response:?}");
    assert_eq!(
        result["close"]["binding_retirement"]["state"], "verified",
        "{response:?}"
    );
    assert_eq!(
        result["close"]["route_retirement"]["state"], "verified",
        "{response:?}"
    );
    assert_eq!(
        result["close"]["subscription_retirement"]["state"], "verified",
        "{response:?}"
    );

    let state = server.state.lock().unwrap();
    assert!(!state.workers.contains_key(peer));
    assert!(state.workers.contains_key(other));
    assert!(state.tasks.contains_key("self-open-task"));
    assert!(state.msgs.contains_key("self-pending-message"));
    assert_eq!(
        state.notification_subscriptions["self-subscription"].status,
        "cancelled"
    );
    assert!(state
        .global
        .lookup_current_thread_route(
            &binding.session_id.clone().unwrap(),
            &binding.native_thread_id.clone().unwrap()
        )
        .is_none());
    assert!(state
        .global
        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
        .is_none());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
