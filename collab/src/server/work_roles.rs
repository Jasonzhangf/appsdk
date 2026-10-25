// Relationship changes and their advisory mail share the caller's journal
// transaction. Existing mailbox/wake owners handle delivery and consumption.
fn relationship_notice_events(
    state: &State, scope: &str, recipient: &str, subject: &str, body: String,
) -> Vec<Event> {
    let now = now_ms();
    let id = format!("relationship-{}", rand::random::<u128>());
    let mut events = vec![Event::Sent { msg: Message {
        id: id.clone(), from: "daemon".into(), to: recipient.into(),
        mtype: "notification".into(), subject: Some(subject.into()), body,
        in_reply_to: None, created_ms: now, state: "pending".into(),
        wake_attempt_count: 0, last_wake_attempt_ms: 0, retry_attempted: false,
    } }, Event::DeliveryMode {
        msg_id: id.clone(), mode: "explicit-notification".into(), source_thread_id: None,
    }];
    if let Some(holder) = state.name_occupancies.get(scope)
        .and_then(|names| names.values().find(|holder| holder.worker_id == recipient)) {
        events.push(Event::MailboxNameAddressed { message_id: id.clone(),
            address: name_mailbox::MailboxNameAddress { project_scope: scope.into(), name: holder.name.clone() } });
    }
    if let Some(subscription) = state.matching_subscription(recipient, "direct-message", None, now) {
        events.push(Event::WakeBound { message_id: id, subscription_id: subscription.id.clone() });
    }
    events
}

fn deliver_relationship_notices(server: &Server, events: &[Event]) -> Vec<serde_json::Value> {
    events.iter().filter_map(|event| {
        let Event::Sent { msg } = event else { return None; };
        let (subscription, method) = {
            let state = server.state.lock().unwrap();
            (state.matching_subscription(&msg.to, "direct-message", None, now_ms()).cloned(),
             notification_method_for_worker(&state, &msg.to))
        };
        let attempt = match &subscription {
            Some(subscription) => attempt_notification_detailed_with_at(server, &msg.id, &subscription.id, now_ms()),
            None => NotificationAttempt::NotAttempted(NOTIFICATION_SUBSCRIPTION_MISSING_ERROR.into()),
        };
        Some(serde_json::to_value(notification_send_response(
            json!({"msg_id":msg.id,"to":msg.to,"consumed":false}),
            subscription.is_some(), method, &attempt,
        )).expect("notification receipt serializes"))
    }).collect()
}

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
    let mut notices = Vec::new();
    if !unchanged {
        let event = Event::NameWorkRolesUpdated {
            project_scope: scope.into(),
            record: crate::board::NameWorkRoles {
                name: name.clone(),
                work_roles: normalized.clone(),
            },
        };
        let recipient = if own_name.as_deref() == Some(name.as_str()) {
            current_master_worker_id(&state, Some(&route))
        } else {
            state.name_occupancy(scope, &name).map(|holder| holder.worker_id.clone())
        };
        if let Some(recipient) = recipient.filter(|recipient| recipient != &actor && state.workers.contains_key(recipient)) {
            notices = relationship_notice_events(&state, scope, &recipient, "work-roles-changed",
                format!("{name} 的工作专长已变更为 {:?}。读取 collab context / collab board show 获取当前关系。只调整受影响的新任务或交接；已接任务的归属与验收保持。无需仅因角色变化改模板或向用户例行汇报。", normalized));
        }
        let mut events = vec![event];
        events.extend(notices.clone());
        if let Err(error) = server.commit_locked_checked(&mut state, &events) {
            return Resp::err(format!("WORK_ROLES_DURABILITY_FAILED: {error}"));
        }
    }
    drop(state);
    let notifications = deliver_relationship_notices(server, &notices);
    Resp::data(
        json!({"name":name,"work_roles":normalized,"outcome":if unchanged {"unchanged"} else {"updated"},"notifications":notifications}),
    )
}

#[cfg(test)]
mod work_role_state_tests {
    use super::*;

    #[test]
    fn project_reset_removes_local_specialties_and_defaults_but_keeps_other_scope() {
        let mut state = State::default();
        for project_scope in ["/project", "/other-project"] {
            state
                .apply_checked(&Event::NameWorkRolesUpdated {
                    project_scope: project_scope.into(),
                    record: crate::board::NameWorkRoles {
                        name: "Alice".into(),
                        work_roles: vec!["designer".into()],
                    },
                })
                .unwrap();
        }
        state
            .apply_checked(&Event::ProjectWorkPolicyUpdated {
                work_kind: "frontend".into(),
                policy: crate::board::WorkPolicy {
                    executor_name: Some("Alice".into()),
                    ..Default::default()
                },
            })
            .unwrap();
        state
            .apply_checked(&Event::ProjectResetStarted {
                project_scope: "/project".into(),
                clear_local: true,
                worker_ids: vec![],
            })
            .unwrap();
        assert!(state.name_work_roles("/project", "Alice").is_empty());
        assert_eq!(
            state.name_work_roles("/other-project", "Alice"),
            ["designer"]
        );
        assert!(state.project_work_policy.is_empty());
    }

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
