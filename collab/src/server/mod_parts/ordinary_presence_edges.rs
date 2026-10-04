fn record_ordinary_peer_presence_edges(server: &Server, worker_filter: Option<&str>) {
    let master = match live_master_worker_snapshot(server) {
        Ok(Some(master)) => master,
        _ => return,
    };
    let observations: Vec<(WorkerRec, &'static str)> = {
        let state = server.state.lock().unwrap();
        state
            .workers
            .values()
            .filter(|worker| worker_filter.is_none_or(|id| id == worker.id))
            .filter(|worker| {
                worker
                    .transport
                    .as_ref()
                    .is_some_and(|transport| transport.kind == TransportKind::Tmux)
            })
            .cloned()
            .collect::<Vec<_>>()
    }
    .into_iter()
    .filter_map(|worker| {
        ordinary_peer_presence_label(worker_presence(server, &worker)).map(|label| (worker, label))
    })
    .collect();

    for (worker, observed_presence) in observations {
        let now = now_ms();
        let mut state = server.state.lock().unwrap();
        let Some(current_worker) = state.workers.get(&worker.id) else {
            continue;
        };
        if current_worker != &worker || is_managed_subagent(&state, &worker.id) {
            continue;
        }
        let route_scope = match server_route_scope(server, &state) {
            Ok(route_scope) => route_scope,
            Err(_) => continue,
        };
        if current_master_worker_id(&state, route_scope.as_ref()).as_deref()
            != Some(master.id.as_str())
            || state.workers.get(&master.id) != Some(&master)
        {
            continue;
        }
        if master.id == worker.id {
            continue;
        }
        let old = state
            .keepalives
            .get(&worker.id)
            .cloned()
            .unwrap_or_default();
        if old.notified_presence == observed_presence {
            continue;
        }

        let previous_presence = old.notified_presence.clone();
        let mut record = old;
        record.notified_presence = observed_presence.into();
        let mut events = vec![Event::KeepaliveUpdated {
            worker_id: worker.id.clone(),
            record,
        }];
        if !previous_presence.is_empty() {
            let offline = observed_presence == "offline";
            events.push(Event::MasterWakeSignal {
                signal: if offline {
                    state::MasterWakeSignal::WorkerUnresponsive {
                        worker_id: worker.id.clone(),
                    }
                } else {
                    state::MasterWakeSignal::WorkerRecovered {
                        worker_id: worker.id.clone(),
                    }
                },
                at_ms: now,
            });
            if server.config.notifications.enabled {
                if let Some(subscription) = state
                    .matching_subscription(&master.id, "direct-message", None, now)
                    .cloned()
                {
                    let message_id = gen_msg_id();
                    events.push(Event::Sent {
                        msg: Message {
                            id: message_id.clone(),
                            from: "collab-server".into(),
                            to: master.id.clone(),
                            mtype: "notify".into(),
                            subject: Some(if offline {
                                format!("worker-unresponsive: {}", worker.id)
                            } else {
                                format!("worker-recovered: {}", worker.id)
                            }),
                            body: if offline {
                                format!(
                                    "Worker {} changed from online to offline. Action required: inspect its durable tasks with `collab task status` and mailbox with `collab inbox`; reassign or close stale work only with evidence.",
                                    worker.id
                                )
                            } else {
                                format!(
                                    "Worker {} changed from offline to online. Action required: resume eligible assigned work or refresh scheduling evidence before dispatch.",
                                    worker.id
                                )
                            },
                            in_reply_to: None,
                            created_ms: now,
                            state: "pending".into(),
                            wake_attempt_count: 0,
                            last_wake_attempt_ms: 0,
                    retry_attempted: false,
                        },
                    });
                    events.push(Event::WakeBound {
                        message_id,
                        subscription_id: subscription.id,
                    });
                }
            }
        }
        server.commit_locked(&mut state, &events);
    }
}
