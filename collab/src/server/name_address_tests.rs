use super::*;
use crate::server::name_state::{AgentPresence, AgentPresenceRecord, NameOccupancy};
use crate::server::peer_tests::test_server;

fn send(server: &Server, to: &str, subject: &str) -> Resp {
    handle_send(
        server,
        "sender".into(),
        to.into(),
        "notify".into(),
        Some(subject.into()),
        "name address fixture".into(),
        None,
        "queued".into(),
    )
}

#[test]
fn name_address_current_holder_and_id_compatibility() {
    let (server, root) = test_server();
    for id in ["sender", "w1", "w2"] {
        let response = crate::server::peer_tests::register_synthetic_peer(&server, id, id);
        assert!(response.ok, "{:?}", response.error);
    }
    let route = server_route_scope(&server, &server.state.lock().unwrap())
        .unwrap()
        .unwrap();
    for name in ["Alice", "Jenny", "Xavier"] {
        for (revision, worker) in [(1, "w1"), (2, "w2")] {
            {
                let mut state = server.state.lock().unwrap();
                // A historical label and an occupancy in another project must not route sends.
                state.worker_names.insert("w1".into(), name.into());
                let holder = NameOccupancy {
                    name: name.into(),
                    worker_id: worker.into(),
                    session_id: worker.into(),
                    occupancy_revision: revision,
                    updated_ms: revision as i64,
                };
                state
                    .name_occupancies
                    .entry(route.project_scope_id.as_str().into())
                    .or_default()
                    .insert(name.into(), holder.clone());
                state
                    .apply_agent_presence(&AgentPresenceRecord {
                        project_scope: route.project_scope_id.as_str().into(),
                        worker_id: worker.into(),
                        session_id: worker.into(),
                        presence: AgentPresence::Unknown,
                        reason: "fixture unknown".into(),
                        observed_ms: 3,
                    })
                    .unwrap();
                let mut other = holder;
                other.worker_id = "sender".into();
                state
                    .name_occupancies
                    .entry("other-project".into())
                    .or_default()
                    .insert(name.into(), other);
            }
            let response = send(&server, name, &format!("{name}-{revision}"));
            assert!(response.ok, "name {name}: {:?}", response.error);
            assert_eq!(response.data["consumed"], false);
            let state = server.state.lock().unwrap();
            let message = &state.msgs[response.data["msg_id"].as_str().unwrap()];
            assert_eq!(message.to, worker);
            assert_eq!(message.state, "pending");
        }
    }
    let response = send(&server, "w1", "direct old ID");
    assert!(response.ok, "{:?}", response.error);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.msgs[response.data["msg_id"].as_str().unwrap()].to,
            "w1"
        );
        assert_eq!(state.msgs.len(), 7);
        assert_eq!(state.msgs.values().filter(|m| m.to == "w1").count(), 4);
    }
    let project_key = canonical_project_key(&server.root).unwrap();
    let response = send(&server, &format!("{project_key}.Xavier"), "project custom name");
    assert!(response.ok, "{:?}", response.error);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.msgs[response.data["msg_id"].as_str().unwrap()].to,
            "w2"
        );
    }
    let missing = send(&server, "Tina", "unoccupied");
    assert!(!missing.ok);
    assert!(missing.error.unwrap().contains("NAME_ADDRESS_UNOCCUPIED"));
    assert_eq!(server.state.lock().unwrap().msgs.len(), 8);
    std::fs::remove_dir_all(root).unwrap();
}
