fn record_ordinary_peer_presence_edges(server: &Server, worker_filter: Option<&str>) {
    // The wake edge belongs to the current grant holder. Reachability of that
    // holder is not consulted: a message to an unreachable master stays
    // durable, and the notification attempt reports its own send failure.
    let master = match current_master_worker_record(server, &server.state.lock().unwrap()) {
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
        let typed = {
            let state = server.state.lock().unwrap();
            worker_agent_facts(server, &state, &worker)
                .0
                .map(|fact| fact.presence)
        };
        // Pane reachability can report a lost endpoint. A registered identity
        // or a reachable shell cannot report an online agent recovery.
        let presence = match typed {
            Some(name_state::AgentPresence::Online) => IdentityPresence::Present,
            Some(name_state::AgentPresence::Offline) => IdentityPresence::Missing,
            Some(name_state::AgentPresence::Unknown) => IdentityPresence::Unknown,
            None => match worker_identity_and_endpoint(server, &worker).1 {
                Some(false) => IdentityPresence::Missing,
                _ => IdentityPresence::Unknown,
            },
        };
        ordinary_peer_presence_label(presence).map(|label| (worker, label))
    })
    .collect();

    for (worker, observed_presence) in observations {
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

        let mut record = old;
        record.notified_presence = observed_presence.into();
        let events = vec![Event::KeepaliveUpdated {
            worker_id: worker.id.clone(),
            record,
        }];
        server.commit_locked_reporting(&mut state, &events);
    }
}
