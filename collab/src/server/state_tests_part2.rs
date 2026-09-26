use super::*;
    #[test]
    fn runtime_binding_replay_keeps_a_legacy_thread_only_route_event_resolvable() {
        // The live host journal shape: route events written before the dual
        // key existed, carrying a native thread but no session id.
        let root = replay_test_root("legacy-thread-only-route-event");
        let journal = root.join(".agent-collab/server/journal.jsonl");
        std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
        let scope = ProjectScopeId::new("/replay-project").unwrap();
        let binding = RuntimeBinding {
            project_scope: scope.clone(),
            app_scope_id: AppServerId::new("appserver-cli").unwrap(),
            agent_id: AgentId::new("agent-legacy").unwrap(),
            runtime_id: RuntimeId::new("runtime-legacy").unwrap(),
            binding_id: BindingId::new("binding-legacy").unwrap(),
            endpoint_generation: 4,
            session_id: None,
            native_thread_id: Some(NativeThreadId::new("thread-legacy-live").unwrap()),
            tmux_endpoint: None,
        };
        let events = vec![
            Event::GlobalProjectRegistered {
                registration: ProjectRegistration::new(
                    scope.clone(),
                    AppServerId::new("appserver-cli").unwrap(),
                )
                .unwrap(),
            },
            Event::GlobalRuntimeBound {
                binding: binding.clone(),
            },
            Event::GlobalCurrentThreadRouteSet {
                binding: binding.clone(),
            },
        ];
        let body = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&journal, format!("{body}\n")).unwrap();

        let replayed = crate::server::replay(&root).expect("legacy route replay must not abort");
        let thread = NativeThreadId::new("thread-legacy-live").unwrap();
        let legacy = replayed
            .global
            .legacy_thread_route_matches(&thread)
            .into_iter()
            .next()
            .cloned()
            .expect("legacy route event must stay resolvable");
        assert_eq!(legacy.agent_id.as_str(), "agent-legacy");
        assert_eq!(legacy.endpoint_generation, 4);
        assert!(legacy.session_id.is_none());
        assert!(replayed.global.current_thread_routes.is_empty());
        replayed.global.validate().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_binding_replay_keeps_ambiguous_legacy_thread_only_routes_as_candidates() {
        let root = replay_test_root("legacy-thread-ambiguous");
        let journal = root.join(".agent-collab/server/journal.jsonl");
        std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
        let scope = ProjectScopeId::new("/replay-project").unwrap();
        let binding = |agent: &str, binding_id: &str, runtime_id: &str| RuntimeBinding {
            project_scope: scope.clone(),
            app_scope_id: AppServerId::new("appserver-cli").unwrap(),
            agent_id: AgentId::new(agent).unwrap(),
            runtime_id: RuntimeId::new(runtime_id).unwrap(),
            binding_id: BindingId::new(binding_id).unwrap(),
            endpoint_generation: 1,
            session_id: None,
            native_thread_id: Some(NativeThreadId::new("thread-shared").unwrap()),
            tmux_endpoint: None,
        };
        let events = vec![
            Event::GlobalProjectRegistered {
                registration: ProjectRegistration::new(
                    scope.clone(),
                    AppServerId::new("appserver-cli").unwrap(),
                )
                .unwrap(),
            },
            Event::GlobalRuntimeBound {
                binding: binding("agent-one", "binding-one", "runtime-one"),
            },
            Event::GlobalRuntimeBound {
                binding: binding("agent-two", "binding-two", "runtime-two"),
            },
        ];
        let body = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&journal, format!("{body}\n")).unwrap();
        // Replay must not abort on two candidates for one thread; the resolver
        // fails closed instead, so both survive as read-only candidates.
        let replayed = crate::server::replay(&root).expect("legacy replay must not abort");
        let thread = NativeThreadId::new("thread-shared").unwrap();
        let mut matches = replayed.global.legacy_thread_route_matches(&thread);
        matches.sort_by(|left, right| left.binding_id.as_str().cmp(right.binding_id.as_str()));
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].binding_id.as_str(), "binding-one");
        assert_eq!(matches[1].binding_id.as_str(), "binding-two");
        replayed.global.validate().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_role_field_is_ignored_on_replay() {
        let mut st = State::default();
        let event: Event = serde_json::from_str(
            r#"{"ev":"Registered","worker":{"id":"legacy","token":"t","pane":"%1","cwd":"/tmp","registered_ms":1,"role":"master"}}"#,
        )
        .unwrap();
        st.apply(&event);
        let worker = serde_json::to_value(&st.workers["legacy"]).unwrap();
        assert!(worker.get("role").is_none());
    }

    #[test]
    fn answered_detection() {
        let mut st = State::default();
        st.apply(&Event::Sent {
            msg: msg("m1", "w2", "request"),
        });
        let mut reply = msg("m2", "w1", "reply");
        reply.in_reply_to = Some("m1".into());
        st.apply(&Event::Sent { msg: reply });
        assert!(st.answered("m1"));
        assert!(!st.answered("m2"));
    }

    #[test]
    fn request_cooldown_uses_only_recent_live_request() {
        let mut st = State::default();
        let mut request = msg("request", "w2", "request");
        request.from = "w1".into();
        request.created_ms = 500;
        st.apply(&Event::Sent { msg: request });

        let (id, _) = st
            .recent_live_request("w1", "w2", 500 + REQUEST_COOLDOWN_MS - 1)
            .expect("recent live request blocks a new send");
        assert_eq!(id, "request");
        assert!(st
            .recent_live_request("w1", "w2", 500 + REQUEST_COOLDOWN_MS)
            .is_none());
    }

    #[test]
    fn wait_cycle_rejects_direct_and_transitive_cycles() {
        let base = |id: &str| TaskRec {
            id: id.into(),
            owner: "worker".into(),
            created_by: "worker".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "waiting".into(),
            next_step: None,
            wait: None,
            created_ms: 0,
            updated_ms: 0,
        };
        let mut tasks = HashMap::new();
        let mut a = base("a");
        a.wait = Some(WaitSpec {
            waiter: "a".into(),
            waiting_for: "b".into(),
            responsible_actor: "worker".into(),
            reason: "resource_conflict".into(),
            deadline_ms: 1,
            resume_on: vec!["resource_released".into()],
            escalation: "resource_owner_and_waiter_recheck".into(),
        });
        tasks.insert("a".into(), a);
        assert!(wait_cycle(&tasks, "b", "a"));
        assert!(!wait_cycle(&tasks, "c", "a"));
    }

    #[test]
    fn latest_reply_supersedes_previous_replies() {
        let mut st = State::default();
        st.apply(&Event::Sent {
            msg: msg("request", "w1", "request"),
        });

        let mut first = msg("reply-1", "w1", "reply");
        first.in_reply_to = Some("request".into());
        first.created_ms = 2;
        st.apply(&Event::Sent { msg: first });
        let stale_replies = st.superseded_replies("request");

        let mut latest = msg("reply-2", "w1", "reply");
        latest.in_reply_to = Some("request".into());
        latest.created_ms = 3;
        st.apply(&Event::Sent { msg: latest });
        st.apply(&Event::Superseded { ids: stale_replies });

        assert!(st.answered("request"));
        assert_eq!(st.msgs["reply-1"].state, "superseded");
        assert_eq!(st.msgs["reply-2"].state, "pending");
        assert_eq!(
            st.inbox_of("w1")
                .iter()
                .filter(|m| m.mtype == "reply")
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["reply-2"]
        );
    }

    #[test]
    fn notification_subscription_is_explicit_exact_and_one_shot() {
        let mut state = State::default();
        state.apply(&Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: "sub-release".into(),
                worker_id: "waiter".into(),
                event: "resource-released".into(),
                subject: Some("holder-task".into()),
                target: "thread-7".into(),
                method: "appserver".into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: 10_000,
                status: "armed".into(),
                created_ms: 1,
                updated_ms: 1,
                status_reason: None,
            },
        });

        assert!(state
            .matching_subscription("waiter", "resource-released", Some("holder-task"), 9_999)
            .is_some());
        assert!(state
            .matching_subscription("waiter", "resource-released", Some("other-task"), 9_999)
            .is_none());
        assert!(state
            .matching_subscription("waiter", "resource-released", Some("holder-task"), 10_000)
            .is_none());

        state.apply(&Event::NotificationConsumed {
            subscription_id: "sub-release".into(),
            message_id: "message".into(),
            consumed_ms: 8_000,
        });
        assert!(state
            .matching_subscription("waiter", "resource-released", Some("holder-task"), 8_001)
            .is_none());
    }

    #[test]
    fn wake_binding_is_control_state_not_message_payload() {
        let mut state = State::default();
        state.apply(&Event::Sent {
            msg: msg("message", "waiter", "notify"),
        });
        state.apply(&Event::WakeBound {
            message_id: "message".into(),
            subscription_id: "subscription".into(),
        });

        assert_eq!(state.wake_bindings["message"], "subscription");
        assert!(serde_json::to_value(&state.msgs["message"])
            .unwrap()
            .get("subscription_id")
            .is_none());
    }

    #[test]
    fn periodic_subscription_consumes_exactly_its_repeat_count() {
        let mut state = State::default();
        state.apply(&Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: "sub-periodic".into(),
                worker_id: "waiter".into(),
                event: "deadline".into(),
                subject: Some("timer".into()),
                target: "thread-7".into(),
                method: "appserver".into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: Some(1_000),
                repeat_count: 3,
                fired_count: 0,
                expires_ms: 10_000,
                status: "armed".into(),
                created_ms: 1,
                updated_ms: 1,
                status_reason: None,
            },
        });
        for count in 1..=3 {
            state.apply(&Event::NotificationConsumed {
                subscription_id: "sub-periodic".into(),
                message_id: format!("m{count}"),
                consumed_ms: count * 1_000,
            });
            if count < 3 {
                assert_eq!(
                    state.notification_subscriptions["sub-periodic"].status,
                    "armed"
                );
            }
        }
        assert_eq!(
            state.notification_subscriptions["sub-periodic"].status,
            "consumed"
        );
        assert_eq!(
            state.notification_subscriptions["sub-periodic"].fired_count,
            3
        );
    }

    #[test]
    fn cancelled_periodic_subscription_does_not_rearm_on_consume_or_replay() {
        let subscription_id = "sub-cancelled-periodic";
        let message_id = "message-cancelled-periodic";
        let mut state = State::default();
        state.apply(&Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.into(),
                worker_id: "master".into(),
                event: "deadline".into(),
                subject: Some("timer".into()),
                target: "thread-7".into(),
                method: "appserver".into(),
                trigger_ms: Some(2_000),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(1_000),
                repeat_count: 3,
                fired_count: 0,
                expires_ms: 20_000,
                status: "armed".into(),
                created_ms: 1_000,
                updated_ms: 1_000,
                status_reason: None,
            },
        });
        state.apply(&Event::Sent {
            msg: Message {
                id: message_id.into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notification".into(),
                subject: Some("deadline:timer".into()),
                body: "timer occurrence".into(),
                in_reply_to: None,
                created_ms: 2_000,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        });
        state.apply(&Event::WakeBound {
            message_id: message_id.into(),
            subscription_id: subscription_id.into(),
        });
        state.apply(&Event::Delivered {
            ids: vec![message_id.into()],
        });
        state.apply(&Event::NotificationStatus {
            subscription_id: subscription_id.into(),
            status: "cancelled".into(),
            updated_ms: 2_500,
        });

        state.apply(&Event::NotificationConsumed {
            subscription_id: subscription_id.into(),
            message_id: message_id.into(),
            consumed_ms: 2_600,
        });
        assert_eq!(
            state.notification_subscriptions[subscription_id].status,
            "cancelled"
        );
        assert_eq!(
            state.notification_subscriptions[subscription_id].fired_count,
            0
        );

        state.apply(&Event::Acked {
            ids: vec![message_id.into()],
        });
        assert_eq!(
            state.notification_subscriptions[subscription_id].status,
            "cancelled"
        );
        assert_eq!(
            state.notification_subscriptions[subscription_id].fired_count,
            0
        );

        let mut replayed = State::default();
        for event in state.snapshot_events() {
            replayed.apply(&event);
        }
        let replayed_subscription = &replayed.notification_subscriptions[subscription_id];
        assert_eq!(replayed_subscription.status, "cancelled");
        assert_eq!(replayed_subscription.fired_count, 0);
        assert_eq!(replayed.msgs[message_id].state, "read");
    }

    #[test]
    fn periodic_subscription_persists_fixed_absolute_cursor_across_replay() {
        let subscription = |id: &str, trigger_ms: Option<i64>| NotificationSubscription {
            id: id.into(),
            worker_id: "master".into(),
            event: "deadline".into(),
            subject: Some("periodic".into()),
            target: "thread-7".into(),
            method: "appserver".into(),
            trigger_ms,
            trigger_times_ms: Vec::new(),
            interval_ms: Some(1_000),
            repeat_count: 3,
            fired_count: 0,
            expires_ms: 20_000,
            status: "armed".into(),
            created_ms: 1_000,
            updated_ms: 1_000,
            status_reason: None,
        };

        let mut none_state = State::default();
        none_state.apply(&Event::NotificationSubscribed {
            subscription: subscription("sub-periodic-none", None),
        });
        let mut none_cursor = Vec::new();
        for count in 1..=3 {
            none_state.apply(&Event::NotificationConsumed {
                subscription_id: "sub-periodic-none".into(),
                message_id: format!("none-{count}"),
                consumed_ms: 1_000 + (count as i64 * 1_000),
            });
            if count < 3 {
                none_cursor
                    .push(none_state.notification_subscriptions["sub-periodic-none"].trigger_ms);
            }
        }
        assert_eq!(none_cursor, vec![Some(3_000), Some(4_000)]);

        let mut seeded_state = State::default();
        seeded_state.apply(&Event::NotificationSubscribed {
            subscription: subscription("sub-periodic-seeded", Some(2_000)),
        });
        for count in 1..=2 {
            seeded_state.apply(&Event::NotificationConsumed {
                subscription_id: "sub-periodic-seeded".into(),
                message_id: format!("seeded-{count}"),
                consumed_ms: 1_000 + (count as i64 * 1_000),
            });
            assert_eq!(
                seeded_state.notification_subscriptions["sub-periodic-seeded"].trigger_ms,
                Some(2_000 + count as i64 * 1_000)
            );
        }

        let mut replayed = State::default();
        for event in seeded_state.snapshot_events() {
            replayed.apply(&event);
        }
        assert_eq!(
            replayed.notification_subscriptions["sub-periodic-seeded"].trigger_ms,
            seeded_state.notification_subscriptions["sub-periodic-seeded"].trigger_ms
        );
        assert_eq!(
            replayed.notification_subscriptions["sub-periodic-seeded"].fired_count,
            seeded_state.notification_subscriptions["sub-periodic-seeded"].fired_count
        );
    }

    #[test]
    fn worker_close_tombstone_replays_without_deleting_re_registration() {
        let worker = |id: &str, token: &str, registered_ms: i64| WorkerRec {
            id: id.into(),
            token: token.into(),
            cwd: "/tmp/project".into(),
            registered_ms,
            transport: None,
        };
        let mut state = State::default();
        state.apply(&Event::WorkerClosed {
            worker_id: "peer-a".into(),
            closed_by: "master".into(),
            reason: "confirmed offline".into(),
            snapshot_captured_ms: Some(10),
            at_ms: 20,
        });
        state.apply(&Event::Registered {
            worker: worker("peer-a", "token-new", 30),
        });

        let mut replayed = State::default();
        for event in state.snapshot_events() {
            replayed.apply(&event);
        }
        assert_eq!(replayed.workers["peer-a"].token, "token-new");
        assert!(!replayed.worker_closures.contains_key("peer-a"));
        assert!(!replayed.worker_snapshots.contains_key("peer-a"));
    }

    #[test]
    fn ack_consumes_scheduled_occurrence_once_and_replay_preserves_cursor() {
        let subscription_id = "sub-ack-periodic";
        let message_id = "message-ack-periodic";
        let mut state = State::default();
        state.apply(&Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.into(),
                worker_id: "master".into(),
                event: "deadline".into(),
                subject: Some("ack-periodic".into()),
                target: "thread-7".into(),
                method: "appserver".into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: Some(1_000),
                repeat_count: 3,
                fired_count: 0,
                expires_ms: 20_000,
                status: "armed".into(),
                created_ms: 1_000,
                updated_ms: 1_000,
                status_reason: None,
            },
        });
        state.apply(&Event::Sent {
            msg: Message {
                id: message_id.into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notification".into(),
                subject: Some("deadline:ack-periodic".into()),
                body: "scheduled occurrence".into(),
                in_reply_to: None,
                created_ms: 2_000,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        });
        state.apply(&Event::WakeBound {
            message_id: message_id.into(),
            subscription_id: subscription_id.into(),
        });
        state.apply(&Event::Delivered {
            ids: vec![message_id.into()],
        });
        state.apply(&Event::Acked {
            ids: vec![message_id.into()],
        });

        let subscription = &state.notification_subscriptions[subscription_id];
        assert_eq!(subscription.fired_count, 1);
        assert_eq!(subscription.trigger_ms, Some(3_000));
        assert_eq!(subscription.status, "armed");

        // A duplicate ACK sees the same read occurrence and cannot advance the
        // durable cursor a second time.
        state.apply(&Event::Acked {
            ids: vec![message_id.into()],
        });
        let subscription = &state.notification_subscriptions[subscription_id];
        assert_eq!(subscription.fired_count, 1);
        assert_eq!(subscription.trigger_ms, Some(3_000));

        let mut replayed = State::default();
        for event in state.snapshot_events() {
            replayed.apply(&event);
        }
        let replayed_subscription = &replayed.notification_subscriptions[subscription_id];
        assert_eq!(replayed_subscription.fired_count, 1);
        assert_eq!(replayed_subscription.trigger_ms, Some(3_000));
        assert_eq!(replayed.msgs[message_id].state, "read");
    }

    #[test]
    fn ack_after_timer_consumption_does_not_double_consume() {
        let subscription_id = "sub-ack-timer";
        let message_id = "message-ack-timer";
        let mut state = State::default();
        state.apply(&Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.into(),
                worker_id: "master".into(),
                event: "deadline".into(),
                subject: Some("ack-timer".into()),
                target: "thread-7".into(),
                method: "appserver".into(),
                trigger_ms: Some(2_000),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(1_000),
                repeat_count: 3,
                fired_count: 0,
                expires_ms: 20_000,
                status: "armed".into(),
                created_ms: 1_000,
                updated_ms: 1_000,
                status_reason: None,
            },
        });
        state.apply(&Event::Sent {
            msg: Message {
                id: message_id.into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notification".into(),
                subject: Some("deadline:ack-timer".into()),
                body: "timer occurrence".into(),
                in_reply_to: None,
                created_ms: 2_000,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        });
        state.apply(&Event::WakeBound {
            message_id: message_id.into(),
            subscription_id: subscription_id.into(),
        });
        state.apply(&Event::Delivered {
            ids: vec![message_id.into()],
        });
        state.apply(&Event::NotificationConsumed {
            subscription_id: subscription_id.into(),
            message_id: message_id.into(),
            consumed_ms: 2_001,
        });
        assert_eq!(
            state.notification_subscriptions[subscription_id].fired_count,
            1
        );
        assert_eq!(
            state.notification_subscriptions[subscription_id].trigger_ms,
            Some(3_000)
        );

        state.apply(&Event::Acked {
            ids: vec![message_id.into()],
        });
        assert_eq!(
            state.notification_subscriptions[subscription_id].fired_count,
            1
        );
        assert_eq!(
            state.notification_subscriptions[subscription_id].trigger_ms,
            Some(3_000)
        );
    }

    #[test]
    fn ack_on_reusable_direct_message_does_not_consume_subscription() {
        let subscription_id = "sub-ack-direct";
        let message_id = "message-ack-direct";
        let mut state = State::default();
        state.apply(&Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.into(),
                worker_id: "worker".into(),
                event: "direct-message".into(),
                subject: None,
                target: "thread-7".into(),
                method: "appserver".into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: 20_000,
                status: "armed".into(),
                created_ms: 1_000,
                updated_ms: 1_000,
                status_reason: None,
            },
        });
        state.apply(&Event::Sent {
            msg: Message {
                id: message_id.into(),
                from: "peer".into(),
                to: "worker".into(),
                mtype: "notify".into(),
                subject: Some("direct".into()),
                body: "reusable message".into(),
                in_reply_to: None,
                created_ms: 2_000,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        });
        state.apply(&Event::WakeBound {
            message_id: message_id.into(),
            subscription_id: subscription_id.into(),
        });
        state.apply(&Event::Delivered {
            ids: vec![message_id.into()],
        });
        state.apply(&Event::Acked {
            ids: vec![message_id.into()],
        });

        let subscription = &state.notification_subscriptions[subscription_id];
        assert_eq!(subscription.fired_count, 0);
        assert_eq!(subscription.status, "armed");
        assert_eq!(state.msgs[message_id].state, "read");
    }
