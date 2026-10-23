fn worker_work_roles(state: &State, project_scope: &str, worker_id: &str) -> Vec<String> {
    state
        .name_occupancies
        .get(project_scope)
        .and_then(|names| {
            names
                .values()
                .find(|entry| entry.worker_id == worker_id)
                .map(|entry| state.name_work_roles(project_scope, &entry.name))
        })
        .unwrap_or_default()
}

fn handle_work_roles(
    server: &Server,
    actor: String,
    token: String,
    name: Option<String>,
    roles: Vec<String>,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &actor, &token) {
        return error;
    }
    let route = match server_route_scope(server, &state) {
        Ok(Some(route)) => route,
        _ => return Resp::err("WORK_ROLES_PROJECT_REQUIRED: project route is not ready"),
    };
    let scope = route.project_scope_id.as_str();
    let own_name = state.name_occupancies.get(scope).and_then(|names| {
        names
            .values()
            .find(|entry| entry.worker_id == actor)
            .map(|entry| entry.name.clone())
    });
    let Some(name) = name.or_else(|| own_name.clone()) else {
        return Resp::err("WORK_ROLES_NAME_REQUIRED: restore a stable name first");
    };
    if own_name.as_deref() != Some(name.as_str()) {
        match current_master_holder(server, &state) {
            Ok(Some(master)) if master == actor => {},
            Ok(_) => return Resp::err("WORK_ROLES_MASTER_REQUIRED: only the current master may set another name's specialties"),
            Err(error) => return Resp::err(error),
        }
    }
    let known = state.name_occupancy(scope, &name).is_some()
        || state
            .name_occupancy_revisions
            .get(scope)
            .is_some_and(|names| names.contains_key(&name));
    if !known {
        return Resp::err("WORK_ROLES_NAME_NOT_FOUND: unknown stable name");
    }
    let mut normalized = Vec::new();
    for role in roles {
        let role = role.trim().to_lowercase();
        if role.is_empty() || role.chars().any(char::is_control) || role.contains(',') {
            return Resp::err("WORK_ROLES_INVALID: each specialty must be a nonempty value without control characters or commas");
        }
        normalized.push(role);
    }
    normalized.sort();
    normalized.dedup();
    let unchanged = state.name_work_roles(scope, &name) == normalized;
    if !unchanged {
        let event = Event::NameWorkRolesUpdated {
            project_scope: scope.into(),
            record: crate::board::NameWorkRoles {
                name: name.clone(),
                work_roles: normalized.clone(),
            },
        };
        if let Err(error) = server.commit_locked_checked(&mut state, &[event]) {
            return Resp::err(format!("WORK_ROLES_DURABILITY_FAILED: {error}"));
        }
    }
    Resp::data(
        json!({"name":name,"work_roles":normalized,"outcome":if unchanged {"unchanged"} else {"updated"}}),
    )
}

#[cfg(test)]
mod work_role_state_tests {
    use super::*;

    #[test]
    fn specialties_follow_name_session_handoff_and_survive_compaction() {
        let mut state = State::default();
        for (worker, session, revision) in [("old", "s1", 1), ("current", "s2", 2)] {
            state
                .apply_checked(&Event::NameResolutionCommitted {
                    project_scope: "/project".into(),
                    occupancy: name_state::NameOccupancy {
                        name: "Alice".into(),
                        worker_id: worker.into(),
                        session_id: session.into(),
                        occupancy_revision: revision,
                        updated_ms: revision as i64,
                    },
                    assignments: vec![],
                })
                .unwrap();
            if revision == 1 {
                state
                    .apply_checked(&Event::NameWorkRolesUpdated {
                        project_scope: "/project".into(),
                        record: crate::board::NameWorkRoles {
                            name: "Alice".into(),
                            work_roles: vec!["designer".into(), "reviewer".into()],
                        },
                    })
                    .unwrap();
            }
        }
        assert!(worker_work_roles(&state, "/project", "old").is_empty());
        assert_eq!(
            worker_work_roles(&state, "/project", "current"),
            ["designer", "reviewer"]
        );
        let mut replay = State::default();
        for event in state.snapshot_events() {
            replay.apply_checked(&event).unwrap();
        }
        assert_eq!(
            worker_work_roles(&replay, "/project", "current"),
            ["designer", "reviewer"]
        );
        assert!(replay.name_work_roles("/other-project", "Alice").is_empty());
        replay
            .apply_checked(&Event::NameWorkRolesUpdated {
                project_scope: "/project".into(),
                record: crate::board::NameWorkRoles {
                    name: "Alice".into(),
                    work_roles: vec![],
                },
            })
            .unwrap();
        assert!(worker_work_roles(&replay, "/project", "current").is_empty());
        assert!(worker_work_roles(&state, "/other-project", "current").is_empty());
    }
}
