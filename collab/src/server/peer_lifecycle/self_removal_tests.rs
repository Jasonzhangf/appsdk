use super::*;
use crate::identity::Identity;
use crate::server::peer_tests::{register, register_synthetic_peer, test_server};

fn project_context(root: &Path) -> ProjectContext {
    ProjectContext::for_registered_root_with_app(root, AppServerId::new("tui-default").unwrap())
        .unwrap()
}

fn open_task(owner: &str, id: &str) -> state::TaskRec {
    state::TaskRec {
        id: id.into(),
        owner: owner.into(),
        created_by: owner.into(),
        feature_id: None,
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: "P1".into(),
        status: "working".into(),
        next_step: Some("continue after control-plane removal".into()),
        wait: None,
        isolation_mode: None,
        created_ms: now_ms(),
        updated_ms: now_ms(),
    }
}

fn binding_for(server: &Server, worker_id: &str) -> crate::server::RuntimeBinding {
    server
        .state
        .lock()
        .unwrap()
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == worker_id)
        .cloned()
        .unwrap_or_else(|| panic!("current binding for {worker_id}"))
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
    assert!(register(&server, peer, "%self").ok);
    assert!(register_synthetic_peer(&server, other, "%other").ok);
    crate::server::peer_tests::promote_master(
        &server,
        peer,
        "user approved current-pane self removal",
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
    let actual_session = binding
        .session_id
        .as_ref()
        .expect("self binding session")
        .as_str()
        .to_owned();
    let different_session = "explicit-original-session";
    assert_ne!(
        different_session, actual_session,
        "the synthetic negative must be a different session"
    );
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
                isolation_mode: None,
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
                periodic_declared: false,
                goal_content: None,
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

    let template = server.state.lock().unwrap().msgs["self-pending-message"].clone();
    let mut delivered = template.clone();
    delivered.id = "self-delivered-message".into();
    delivered.state = "delivered".into();
    let mut addressed = template.clone();
    addressed.id = "self-other-name-message".into();
    let other_address = super::super::name_mailbox::MailboxNameAddress {
        project_scope: binding.project_scope.as_str().to_owned(),
        name: "Bob".into(),
    };
    let mut pinned = template;
    pinned.id = "self-session-pinned-message".into();
    let mut matching = server.state.lock().unwrap().msgs["self-pending-message"].clone();
    matching.id = "self-session-matching-message".into();
    server.commit(&[
        state::Event::Sent { msg: delivered },
        state::Event::Sent { msg: addressed },
        state::Event::MailboxNameAddressed {
            message_id: "self-other-name-message".into(),
            address: other_address.clone(),
        },
        state::Event::Sent { msg: pinned },
        state::Event::Sent { msg: matching },
        state::Event::TargetSessionBound {
            message_id: "self-session-matching-message".into(),
            target_session: actual_session.clone(),
        },
        state::Event::TargetSessionBound {
            message_id: "self-session-pinned-message".into(),
            target_session: different_session.into(),
        },
    ]);

    let request = self_close_request(&server, &context, peer, "self-with-open-work");
    let response = handle(&server, Some(&context), request, None);
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
        state.mailbox_name_addresses["self-pending-message"].name,
        "Jenny"
    );
    assert_eq!(
        state.mailbox_name_addresses["self-delivered-message"].name,
        "Jenny"
    );
    assert_eq!(
        state.mailbox_name_addresses["self-other-name-message"],
        other_address
    );
    assert!(!state
        .mailbox_name_addresses
        .contains_key("self-session-pinned-message"));
    assert_eq!(
        state.mailbox_name_addresses["self-session-matching-message"].name,
        "Jenny"
    );
    assert_eq!(
        state.target_sessions["self-session-matching-message"],
        actual_session
    );
    assert_eq!(
        state.target_sessions["self-session-pinned-message"],
        different_session
    );
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

#[test]
fn self_close_preserves_a_task_linked_subscription_for_the_next_owner() {
    let (server, root) = test_server();
    let context = project_context(&root);
    let peer = "peer-task-owner";
    let next = "peer-task-next";
    assert!(register_synthetic_peer(&server, peer, "%task-owner").ok);
    assert!(register_synthetic_peer(&server, next, "%task-next").ok);

    let binding = binding_for(&server, peer);
    let endpoint = binding.tmux_endpoint.clone().expect("tmux endpoint");
    let now = now_ms();
    let mut task = open_task(peer, "task-linked-subscription");
    server.commit(&[
        state::Event::TaskUpdated { task: task.clone() },
        state::Event::BoardDetailsChanged {
            task_id: task.id.clone(),
            details: crate::board::BoardTaskDetails {
                title: "task-linked subscription".into(),
                description: "preserve the task wake while no holder exists".into(),
                delivery_condition: "deliver once".into(),
                test_condition: "verify continuation".into(),
                long_horizon: Some(crate::board::LongHorizonTaskSpec {
                    content: "continue the same task".into(),
                    interval_ms: 60_000,
                    repeat_count: 8,
                    ttl_seconds: 86_400,
                }),
                revision: 2,
                public_visibility: true,
                ..crate::board::BoardTaskDetails::default()
            },
        },
        state::Event::NotificationSubscribed {
            subscription: state::NotificationSubscription {
                id: "task-linked-sub".into(),
                worker_id: peer.into(),
                event: "deadline".into(),
                subject: Some(format!("goal:task:{}", task.id)),
                target: endpoint.pane_id.clone(),
                method: "tmux".into(),
                trigger_ms: Some(now + 60_000),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(60_000),
                repeat_count: 8,
                fired_count: 3,
                expires_ms: now + 86_400_000,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: true,
                goal_content: Some(state::GoalScheduleBinding {
                    source: Some(state::GoalScheduleContent::BoardTask {
                        task_id: task.id.clone(),
                    }),
                    ttl_seconds: 86_400,
                }),
            },
        },
        state::Event::Sent {
            msg: state::Message {
                id: "task-linked-wake".into(),
                from: "daemon".into(),
                to: peer.into(),
                mtype: "notification".into(),
                subject: Some(format!("goal:task:{}", task.id)),
                body: "continue the task".into(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        state::Event::WakeBound {
            message_id: "task-linked-wake".into(),
            subscription_id: "task-linked-sub".into(),
        },
    ]);

    let response = handle(
        &server,
        Some(&context),
        self_close_request(&server, &context, peer, "self-with-task-subscription"),
        None,
    );
    assert_eq!(
        response.data["result"]["outcome"], "complete",
        "{response:?}"
    );
    {
        let state = server.state.lock().unwrap();
        let subscription = &state.notification_subscriptions["task-linked-sub"];
        assert_eq!(subscription.status, "waiting-owner");
        assert_eq!(subscription.fired_count, 3);
        assert_eq!(subscription.repeat_count, 8);
        assert_eq!(subscription.interval_ms, Some(60_000));
        assert_eq!(
            subscription.goal_content,
            Some(state::GoalScheduleBinding {
                source: Some(state::GoalScheduleContent::BoardTask {
                    task_id: task.id.clone(),
                }),
                ttl_seconds: 86_400,
            })
        );
        assert_ne!(state.msgs["task-linked-wake"].state, "pending");
    }

    task.owner = next.into();
    task.updated_ms = now + 1;
    server.commit(&[state::Event::TaskUpdated { task }]);
    let state = server.state.lock().unwrap();
    let subscription = &state.notification_subscriptions["task-linked-sub"];
    assert_eq!(subscription.status, "armed");
    assert_eq!(subscription.worker_id, next);
    assert_eq!(subscription.fired_count, 3);
    assert_eq!(subscription.repeat_count, 8);
    assert_eq!(subscription.interval_ms, Some(60_000));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn self_close_removes_the_retired_worker_identity_anchor() {
    let (server, root) = test_server();
    let context = project_context(&root);
    let peer = "peer-self-anchor";
    assert!(register_synthetic_peer(&server, peer, "%self-anchor").ok);

    let identity_dir = server.host_paths.state_root().join("identities").join(peer);
    std::fs::create_dir_all(&identity_dir).unwrap();
    let identity = Identity {
        worker_id: peer.into(),
        token: format!("token-{peer}"),
        project_scope: Some(context.project_scope.clone()),
        runtime: None,
        transport: None,
    };
    let identity_path = identity_dir.join("identity.json");
    std::fs::write(&identity_path, serde_json::to_vec(&identity).unwrap()).unwrap();

    let request = self_close_request(&server, &context, peer, "self-removes-anchor");
    let response = handle(&server, Some(&context), request, None);
    assert_eq!(
        response.data["result"]["outcome"], "complete",
        "{response:?}"
    );
    assert!(
        !identity_path.exists(),
        "self-unregister must remove the retired worker's persisted identity anchor"
    );

    std::fs::remove_dir_all(root).unwrap();
}
