// Relationship changes and advisory mail share the current-state transaction.
fn relationship_notice_events(
    state: &State, control: &binding_slot::BindingControlSet, route: &RouteScope,
    recipient: &str, subject: &str, body: String,
) -> Result<Vec<Event>, String> {
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
    if let Some(slot) = control.current_for_worker(&route.project_scope_id, &route.app_scope_id, recipient)? {
        events.push(Event::MailboxNameAddressed { message_id: id.clone(),
            address: name_mailbox::MailboxNameAddress { project_scope: route.project_scope_id.as_str().into(), name: slot.identity_key.name.clone() } });
    }
    if let Some(subscription) = state.matching_subscription(recipient, "direct-message", None, now) {
        events.push(Event::WakeBound { message_id: id, subscription_id: subscription.id.clone() });
    }
    Ok(events)
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

fn worker_work_roles(state: &State, control: &binding_slot::BindingControlSet, route: Option<&RouteScope>, worker_id: &str) -> Result<Vec<String>, String> {
    let Some(route) = route else { return Ok(Vec::new()); };
    Ok(control.current_for_worker(&route.project_scope_id, &route.app_scope_id, worker_id)?
        .map(|slot| state.name_work_roles(route.project_scope_id.as_str(), &slot.identity_key.name)).unwrap_or_default())
}

fn handle_work_roles(
    server: &Server,
    actor: String,
    token: String,
    name: Option<String>,
    roles: Vec<String>,
) -> Resp {
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &actor, &token) {
        return error;
    }
    let route = match server_route_scope(server, &state) {
        Ok(Some(route)) => route,
        _ => return Resp::err("WORK_ROLES_PROJECT_REQUIRED: project route is not ready"),
    };
    let scope = route.project_scope_id.as_str();
    let own_name = match control.current_for_worker(&route.project_scope_id, &route.app_scope_id, &actor) {
        Ok(slot) => slot.map(|slot| slot.identity_key.name.clone()), Err(error) => return Resp::err(error),
    };
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
    let current = match control.current_for_name(&route.project_scope_id, &route.app_scope_id, &name) {
        Ok(current) => current, Err(error) => return Resp::err(error),
    };
    let Some(current) = current else {
        return Resp::err("WORK_ROLES_NAME_NOT_FOUND: unknown stable name");
    };
    let name = current.identity_key.name.clone();
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
            current.active_binding().map(|current| current.binding.agent_id.as_str().to_owned())
        };
        if let Some(recipient) = recipient.filter(|recipient| recipient != &actor && state.workers.contains_key(recipient)) {
            notices = match relationship_notice_events(&state, &control, &route, &recipient, "work-roles-changed",
                format!("{name} 的工作专长已变更为 {:?}。读取 collab context / collab board show 获取当前关系。只调整受影响的新任务或交接；已接任务的归属与验收保持。无需仅因角色变化改模板或向用户例行汇报。", normalized)) {
                Ok(events) => events, Err(error) => return Resp::err(error),
            };
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
    fn specialties_follow_current_map_session_handoff_and_survive_serialization() {
        let mut state = State::default();
        let mut control = binding_slot::BindingControlSet::default();
        let route = RouteScope {
            project_scope_id: ProjectScopeId::new("/project").unwrap(),
            app_scope_id: AppServerId::new("appserver-cli").unwrap(),
        };
        for (worker, session, revision) in [("old", "s1", 1), ("current", "s2", 2)] {
            let thread = format!("thread-{worker}");
            let mut transport = peer_tests::test_appserver_transport(&thread);
            transport.session_id = Some(session.into());
            let binding = RuntimeBinding::new_with_session(
                route.project_scope_id.clone(), route.app_scope_id.clone(),
                AgentId::new(worker).unwrap(), RuntimeId::new(format!("runtime-{worker}")).unwrap(),
                BindingId::new(format!("binding-{worker}")).unwrap(),
                Some(crate::identity::SessionId::new(session).unwrap()),
                Some(NativeThreadId::new(thread).unwrap()),
            ).unwrap();
            control.commit(&binding_slot::IdentityBindingKey::new(route.project_scope_id.clone(), route.app_scope_id.clone(), "Alice"),
                binding_slot::BindingValue::new(binding, transport), None).unwrap();
            state
                .apply_checked(&Event::NameResolutionCommitted {
                    project_scope: "/project".into(),
                    occupancy: name_state::NameOccupancy {
                        name: "Alice".into(),
                        worker_id: worker.into(),
                        session_id: session.into(),
                        occupancy_revision: revision,
                        updated_ms: revision as i64,
                        pane_anchor: None,
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
        assert!(worker_work_roles(&state, &control, Some(&route), "old").unwrap().is_empty());
        assert_eq!(
            worker_work_roles(&state, &control, Some(&route), "current").unwrap(),
            ["designer", "reviewer"]
        );
        let mut loaded: State = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(
            worker_work_roles(&loaded, &control, Some(&route), "current").unwrap(),
            ["designer", "reviewer"]
        );
        assert!(loaded.name_work_roles("/other-project", "Alice").is_empty());
        loaded
            .apply_checked(&Event::NameWorkRolesUpdated {
                project_scope: "/project".into(),
                record: crate::board::NameWorkRoles {
                    name: "Alice".into(),
                    work_roles: vec![],
                },
            })
            .unwrap();
        assert!(worker_work_roles(&loaded, &control, Some(&route), "current").unwrap().is_empty());
        let foreign = RouteScope { project_scope_id: ProjectScopeId::new("/other-project").unwrap(), ..route };
        assert!(worker_work_roles(&state, &control, Some(&foreign), "current").unwrap().is_empty());
    }
}
