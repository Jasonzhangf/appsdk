// Host-wide CEO authority and cross-project coordination. Project reducers
// retain their task and mailbox ownership; only host.state holds the grant.
use crate::ceo::{CeoBootstrapRecord, CeoCommand, CeoGrant};

fn append_ceo_responsibilities(brief: &mut serde_json::Value, rules: &[String]) {
    let Some(current) = brief["responsibilities"].as_array_mut() else {
        return;
    };
    for rule in rules {
        let rule = rule.trim();
        if rule.is_empty() || current.iter().any(|item| item.as_str() == Some(rule)) {
            continue;
        }
        current.push(json!(rule));
    }
}

fn append_ceo_operation(operations: &mut serde_json::Value, operation: serde_json::Value) {
    let Some(existing) = operations.as_array_mut() else {
        return;
    };
    if existing.iter().any(|item| item["action"] == operation["action"]) {
        return;
    }
    existing.push(operation);
}

impl ProjectRuntimeManager {
    fn reconcile_ceo_workspace_reset(&self, context: Option<&ProjectContext>) -> Result<(), Resp> {
        let grant = self.host.state.lock().unwrap().ceo_grant.clone();
        let (Some(grant), Some(context)) = (grant, context) else {
            return Ok(());
        };
        if grant.workspace != context.canonical_root
            || grant.app_scope_id != context.app_scope_id.as_str()
        {
            return Ok(());
        }
        let mut state = self.host.state.lock().unwrap();
        if state.ceo_grant.as_ref() == Some(&grant) {
            self.host
                .commit_locked_checked(&mut state, &[Event::CeoGrantSet { grant: None }])
                .map_err(|e| Resp::err(e.to_string()))?;
        }
        Ok(())
    }

    fn decorate_ceo_context(&self, context: Option<&ProjectContext>, data: &mut serde_json::Value) {
        if data.get("identity").is_some() {
            self.decorate_ceo_snapshot(context, data);
        }
        if let Some(snapshot) = data.get_mut("snapshot") {
            self.decorate_ceo_snapshot(context, snapshot);
        }
        if let Some(snapshot) = data.get_mut("result").and_then(|r| r.get_mut("snapshot")) {
            self.decorate_ceo_snapshot(context, snapshot);
        }
    }

    fn decorate_ceo_snapshot(
        &self,
        context: Option<&ProjectContext>,
        snapshot: &mut serde_json::Value,
    ) {
        let Some(actor) = snapshot["identity"]["worker_id"]
            .as_str()
            .map(str::to_owned)
        else {
            return;
        };
        let status = self.ceo_status(context, &actor);
        snapshot["ceo"] = status.data.clone();
        if status.data["is_current"] == true {
            // The same snapshot can pass the inner Context and outer
            // IdentityContext projections. The metadata card marks that the
            // CEO prose and commands were already composed once.
            if snapshot["role_brief"]["ceo_workflow_template"]["target"]
                .as_str()
                .is_some_and(|target| target == "ceo")
            {
                return;
            }
            let template = &workflow_template_asset().ceo_template;
            // The effective brief is the single home for CEO prose; the card
            // only records where that workflow text came from.
            let brief = &mut snapshot["role_brief"];
            append_workflow_text(brief, "role_task", Some(&template.role_task), "ceo");
            append_ceo_responsibilities(brief, &template.responsibilities);
            append_workflow_text(brief, "next_action", Some(&template.next_action), "ceo");
            append_workflow_text(
                brief,
                "completion_action",
                Some(&template.completion_action),
                "ceo",
            );
            brief["ceo_workflow_template"] = workflow_base_card("ceo");
            let operations = json!([
                {"action":"ceo-progress","command":"collab ceo progress"},
                {"action":"ceo-append-task","command":"collab ceo append-task --project <root> --id <id> --title <title> --description <scope> --delivery-condition <delivery> --test-condition <acceptance>"},
                {"action":"ceo-send","command":"collab ceo send --project <root> --subject <subject> --reply-required <body>"},
                {"action":"ceo-bootstrap","command":"collab ceo bootstrap-master --project <root> --approval <user-approval> --tmux-socket <existing-socket> --tmux-session <existing-session>","requires_approval":true}
            ]);
            if let (Some(existing), Some(extra)) =
                (snapshot["operations"].as_array_mut(), operations.as_array())
            {
                existing.extend(extra.iter().cloned());
            }
        } else if snapshot["role_brief"]["role"] == "master" && !status.data["ceo"].is_null() {
            let rule="Respond to global CEO requests with actual authorized actions, progress or evidenced blockers. Arrange appended tasks on your existing board without abandoning current goals. Use collab ceo reply --subject <subject> <progress>; use --in-reply-to <original-id> only for the requested actual completion. Retain project ownership and pinned executor/reviewer constraints.";
            if let Some(rules) = snapshot["role_brief"]["responsibilities"].as_array_mut() {
                if !rules.iter().any(|item| item.as_str() == Some(rule)) {
                    rules.push(json!(rule));
                }
            }
            append_ceo_operation(
                &mut snapshot["operations"],
                json!({"action":"ceo-reply","command":"collab ceo reply --subject <subject> [--in-reply-to <original-id>] <actual-result>"}),
            );
        }
    }
}

impl ProjectRuntimeManager {
    fn ceo_project_runtimes(
        &self,
        root: &str,
        app: Option<&str>,
    ) -> Result<Vec<Arc<Server>>, Resp> {
        let canonical = std::fs::canonicalize(root)
            .map_err(|e| Resp::err(format!("CEO_PROJECT_INVALID: {e}")))?;
        let entries = self
            .routes
            .lock()
            .unwrap()
            .iter()
            .filter(|((a, _), route)| {
                route.root == canonical && app.is_none_or(|wanted| wanted == a)
            })
            .map(|(key, route)| (key.clone(), route.root.clone(), route.storage_root.clone()))
            .collect::<Vec<_>>();
        entries
            .into_iter()
            .map(|(key, root, storage)| {
                self.ensure_runtime(&key, &root, &storage)
                    .map_err(Resp::err)
            })
            .collect()
    }

    fn ceo_actor(
        &self,
        context: &ProjectContext,
        id: &str,
        token: &str,
    ) -> Result<(Arc<Server>, String), Resp> {
        let runtime = self.select_runtime(context).map_err(Resp::err)?;
        let name = {
            let state = runtime.state.lock().unwrap();
            verify(&state, id, token)?;
            let scope = server_route_scope(&runtime, &state)
                .map_err(Resp::err)?
                .ok_or_else(|| Resp::err("CEO_SOURCE_BINDING_REQUIRED"))?;
            authoritative_send_binding(&state, &scope, id).map_err(Resp::err)?;
            let name = state
                .worker_names
                .get(id)
                .cloned()
                .ok_or_else(|| Resp::err("CEO_SOURCE_NAME_REQUIRED"))?;
            if state
                .name_occupancy(scope.project_scope_id.as_str(), &name)
                .is_none_or(|owner| owner.worker_id != id)
            {
                return Err(Resp::err("CEO_SOURCE_NAME_NOT_CURRENT"));
            }
            name
        };
        Ok((runtime, name))
    }

    fn ceo_grant(&self) -> Result<CeoGrant, Resp> {
        let state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(Resp::err(format!(
                "CEO_AUTHORITY_STORE_UNAVAILABLE: {error}"
            )));
        }
        state
            .ceo_grant
            .clone()
            .ok_or_else(|| Resp::err("CEO_NOT_CONFIGURED"))
    }

    fn require_ceo(&self, context: &ProjectContext, name: &str) -> Result<CeoGrant, Resp> {
        let grant = self.ceo_grant()?;
        if grant.workspace != context.canonical_root
            || grant.app_scope_id != context.app_scope_id.as_str()
            || grant.name != name
        {
            return Err(Resp::err("CEO_AUTHORITY_REQUIRED"));
        }
        Ok(grant)
    }

    fn ceo_holder(&self, grant: &CeoGrant) -> Result<(Arc<Server>, String), Resp> {
        let mut runtimes =
            self.ceo_project_runtimes(&grant.workspace, Some(&grant.app_scope_id))?;
        if runtimes.len() != 1 {
            return Err(Resp::err("CEO_ROUTE_UNAVAILABLE"));
        }
        let runtime = runtimes.remove(0);
        let id = {
            let state = runtime.state.lock().unwrap();
            let holder = state
                .name_occupancy(&grant.workspace, &grant.name)
                .ok_or_else(|| Resp::err("CEO_HOLDER_UNAVAILABLE"))?;
            if !state.workers.contains_key(&holder.worker_id) {
                return Err(Resp::err("CEO_HOLDER_UNAVAILABLE"));
            }
            holder.worker_id.clone()
        };
        Ok((runtime, id))
    }

    fn ceo_target_master(&self, root: &str) -> Result<(Arc<Server>, String), Resp> {
        let mut matches = Vec::new();
        for runtime in self.ceo_project_runtimes(root, None)? {
            let id = current_master_holder(&runtime, &runtime.state.lock().unwrap())
                .map_err(Resp::err)?;
            if let Some(id) = id {
                matches.push((runtime, id));
            }
        }
        if matches.is_empty() {
            return Err(Resp::err("MASTER_BOOTSTRAP_APPROVAL_REQUIRED: no current master; ask the user before ceo bootstrap-master"));
        }
        if matches.len() != 1 {
            return Err(Resp::err("CEO_TARGET_MASTER_AMBIGUOUS"));
        }
        Ok(matches.remove(0))
    }

    fn ceo_status(&self, context: Option<&ProjectContext>, actor: &str) -> Resp {
        let (grant, authority_error) = {
            let state = self.host.state.lock().unwrap();
            (state.ceo_grant.clone(), state.journal_poison.clone())
        };
        let holder = grant
            .as_ref()
            .and_then(|grant| self.ceo_holder(grant).ok())
            .map(|(_, id)| id);
        let is_current = authority_error.is_none()
            && grant.as_ref().is_some_and(|g| {
                context.is_some_and(|c| {
                    c.canonical_root == g.workspace && c.app_scope_id.as_str() == g.app_scope_id
                })
            })
            && holder.as_deref() == Some(actor)
            && !actor.is_empty();
        Resp::data(
            json!({"ceo":grant,"worker_id":holder,"is_current":is_current,"authority_error":authority_error,"default_workspace":self.host.host_paths.state_root().join("workspace")}),
        )
    }

    fn dispatch_ceo(
        &self,
        context: Option<ProjectContext>,
        worker_id: String,
        token: String,
        command: CeoCommand,
    ) -> (Arc<Server>, Resp) {
        let response = if matches!(command, CeoCommand::Status) {
            // Status is public and read-only; an unauthenticated caller never
            // receives an is_current claim based on a supplied worker id.
            let actor = context
                .as_ref()
                .and_then(|ctx| self.ceo_actor(ctx, &worker_id, &token).ok())
                .map(|_| worker_id.clone())
                .unwrap_or_default();
            self.ceo_status(context.as_ref(), &actor)
        } else {
            self.execute_ceo(context.as_ref(), worker_id, token, command)
                .unwrap_or_else(|error| error)
        };
        (self.host.clone(), response)
    }

    fn execute_ceo(
        &self,
        context: Option<&ProjectContext>,
        actor: String,
        token: String,
        command: CeoCommand,
    ) -> Result<Resp, Resp> {
        let context = context.ok_or_else(|| Resp::err("PROJECT_CONTEXT_REQUIRED"))?;
        context
            .validate()
            .map_err(|e| Resp::err(format!("PROJECT_CONTEXT_INVALID: {e}")))?;
        let (source, name) = self.ceo_actor(context, &actor, &token)?;
        if !matches!(command, CeoCommand::Progress { .. })
            && source.state.lock().unwrap().admission_frozen()
        {
            return Err(Resp::err("MIGRATION_ADMISSION_FROZEN"));
        }
        match command {
            CeoCommand::Promote {
                approval,
                workspace,
            } => {
                if approval.trim().is_empty() {
                    return Err(Resp::err("CEO_APPROVAL_REQUIRED"));
                }
                if let Some(workspace) = workspace {
                    let workspace = std::fs::canonicalize(workspace)
                        .map_err(|e| Resp::err(format!("CEO_WORKSPACE_INVALID: {e}")))?;
                    if workspace != Path::new(&context.canonical_root) {
                        return Err(Resp::err("CEO_WORKSPACE_MUST_MATCH_REGISTERED_SOURCE"));
                    }
                }
                let grant = CeoGrant {
                    workspace: context.canonical_root.clone(),
                    app_scope_id: context.app_scope_id.as_str().into(),
                    name,
                    granted_by: actor.clone(),
                    approval,
                    granted_ms: now_ms(),
                };
                self.host
                    .commit_checked(&[Event::CeoGrantSet {
                        grant: Some(grant.clone()),
                    }])
                    .map_err(|e| Resp::err(format!("CEO_DURABILITY_FAILED: {e}")))?;
                // The grant is already durable. Workspace guidance is a bounded
                // follow-up effect: a failure must be reported as partial
                // success and must never roll back the source registration or
                // a grant that the user already approved.
                let mut status = self.ceo_status(Some(context), &actor);
                self.attach_ceo_workspace_guidance(&mut status, &grant);
                Ok(status)
            }
            CeoCommand::Clear { approval } => {
                if approval.trim().is_empty() {
                    return Err(Resp::err("CEO_APPROVAL_REQUIRED"));
                }
                self.host
                    .commit_checked(&[Event::CeoGrantSet { grant: None }])
                    .map_err(|e| Resp::err(format!("CEO_DURABILITY_FAILED: {e}")))?;
                Ok(self.ceo_status(Some(context), &actor))
            }
            CeoCommand::Reply {
                subject,
                body,
                in_reply_to,
                urgent,
            } => {
                if current_master_holder(&source, &source.state.lock().unwrap())
                    .map_err(Resp::err)?
                    .as_deref()
                    != Some(&actor)
                {
                    return Err(Resp::err("CEO_REPLY_PROJECT_MASTER_REQUIRED"));
                }
                if subject.trim().is_empty() || body.trim().is_empty() {
                    return Err(Resp::err("CEO_MESSAGE_CONTENT_REQUIRED"));
                }
                // A configured CEO is required in both cases; a project master
                // must not invent a recipient when no grant exists.
                let grant = self.ceo_grant()?;
                match in_reply_to {
                    // Unsolicited periodic progress: coalesce into the host's
                    // canonical report state. The scheduler owns notification;
                    // this handler never claims a report was sent. The configured
                    // CEO holder must still resolve; a stale grant is not a
                    // destination.
                    None => {
                        let _ = self.ceo_holder(&grant)?;
                        self.ceo_record_unsolicited_progress(&source, subject, body, urgent)
                    }
                    // Solicited reply: preserve the existing durable delivery,
                    // recipient/sender/scope checks, correlation dedup and the
                    // obligation clear. Progress is added only after the reply
                    // itself is durable.
                    Some(original) => {
                        let (target, to) = self.ceo_holder(&grant)?;
                        let delivered = self.ceo_deliver(
                            &source,
                            &actor,
                            &name,
                            &target,
                            &to,
                            subject.clone(),
                            body.clone(),
                            Some(original),
                            None,
                            None,
                            None,
                        );
                        self.attach_solicited_progress(delivered, &source, subject, body, urgent)
                    }
                }
            }
            CeoCommand::BootstrapReady {
                operation_id,
                subject,
                body,
            } => self.ceo_bootstrap_ready(&source, &actor, &name, operation_id, subject, body),
            CeoCommand::DirectMessage { subject, body } => {
                if current_master_holder(&source, &source.state.lock().unwrap())
                    .map_err(Resp::err)?
                    .as_deref()
                    != Some(&actor)
                {
                    return Err(Resp::err(
                        "CEO_DIRECT_MESSAGE_PROJECT_MASTER_REQUIRED: only the current project master may send a direct message to the CEO",
                    ));
                }
                let grant = self.ceo_grant()?;
                let (target, to) = self.ceo_holder(&grant)?;
                self.ceo_deliver(
                    &source, &actor, &name, &target, &to, subject, body, None, None, None, None,
                )
            }
            command => {
                let ceo_grant = self.require_ceo(context, &name)?;
                match command {
                    CeoCommand::Send {
                        project,
                        to,
                        subject,
                        body,
                        in_reply_to,
                        reply_policy,
                    } => {
                        let (target, master) = self.ceo_target_master(&project)?;
                        if let Some(to) = to {
                            let state = target.state.lock().unwrap();
                            if to != master && state.worker_names.get(&master) != Some(&to) {
                                return Err(Resp::err("CEO_TARGET_MUST_BE_CURRENT_MASTER"));
                            }
                        }
                        self.ceo_deliver(
                            &source,
                            &actor,
                            &name,
                            &target,
                            &master,
                            subject,
                            body,
                            in_reply_to,
                            reply_policy,
                            None,
                            None,
                        )
                    }
                    CeoCommand::AppendTask {
                        project,
                        id,
                        title,
                        description,
                        delivery_condition,
                        test_condition,
                        priority,
                        reply_policy,
                    } => {
                        if [
                            &id,
                            &title,
                            &description,
                            &delivery_condition,
                            &test_condition,
                        ]
                        .iter()
                        .any(|s| s.trim().is_empty())
                        {
                            return Err(Resp::err("BOARD_DESCRIPTION_REQUIRED"));
                        }
                        if !["p0", "p1", "p2", "p3", "p4"].contains(&priority.as_str()) {
                            return Err(Resp::err("invalid priority"));
                        }
                        let (target, master) = self.ceo_target_master(&project)?;
                        let now = now_ms();
                        let task = TaskRec {
                            id: id.clone(),
                            owner: master.clone(),
                            created_by: format!("{}@{}", name, context.canonical_root),
                            feature_id: None,
                            worktree_path: None,
                            branch: None,
                            base_commit: None,
                            priority,
                            status: "pending".into(),
                            next_step: None,
                            wait: None,
                            created_ms: now,
                            updated_ms: now,
                        };
                        let details = crate::board::BoardTaskDetails {
                            title: title.clone(),
                            description: description.clone(),
                            delivery_condition: delivery_condition.clone(),
                            test_condition: test_condition.clone(),
                            revision: 1,
                            public_visibility: true,
                            ..Default::default()
                        };
                        let body=format!("Task {id}: {title}\nScope: {description}\nDelivery: {delivery_condition}\nAcceptance: {test_condition}\nArrange this task alongside your existing goals and return the actual delivery or evidenced blocker.");
                        self.ceo_deliver(
                            &source,
                            &actor,
                            &name,
                            &target,
                            &master,
                            format!("Task: {title}"),
                            body,
                            None,
                            Some(reply_policy.unwrap_or(crate::proto::ReplyPolicy::OnCompletion)),
                            Some((task, details)),
                            None,
                        )
                    }
                    CeoCommand::Progress { project } => self.ceo_progress(project),
                    CeoCommand::ReportAck { batch_id } => self.ceo_report_ack(batch_id),
                    CeoCommand::BootstrapPrepare {
                        project,
                        approval,
                        profile,
                        tmux_socket,
                        tmux_session,
                        operation_id,
                    } => self.ceo_bootstrap_prepare(
                        &ceo_grant,
                        project,
                        approval,
                        profile,
                        tmux_socket,
                        tmux_session,
                        operation_id,
                    ),
                    CeoCommand::BootstrapRecord {
                        operation_id,
                        pane_id,
                        outcome,
                        error,
                    } => {
                        self.ceo_bootstrap_record(&ceo_grant, operation_id, pane_id, outcome, error)
                    }
                    CeoCommand::BootstrapCancel {
                        operation_id,
                        approval,
                    } => self.ceo_bootstrap_cancel(operation_id, approval),
                    _ => Err(Resp::err("CEO_COMMAND_INVALID")),
                }
            }
        }
    }
}

impl ProjectRuntimeManager {
    #[allow(clippy::too_many_arguments)]
    fn ceo_deliver(
        &self,
        source: &Arc<Server>,
        actor: &str,
        name: &str,
        target: &Arc<Server>,
        to: &str,
        subject: String,
        body: String,
        in_reply_to: Option<String>,
        policy: Option<crate::proto::ReplyPolicy>,
        task: Option<(TaskRec, crate::board::BoardTaskDetails)>,
        handshake_id: Option<String>,
    ) -> Result<Resp, Resp> {
        if subject.trim().is_empty() || body.trim().is_empty() {
            return Err(Resp::err("CEO_MESSAGE_CONTENT_REQUIRED"));
        }
        let source_project = source.root.to_string_lossy().into_owned();
        let target_project = target.root.to_string_lossy().into_owned();
        let target_name = {
            let state = target.state.lock().unwrap();
            state
                .worker_names
                .get(to)
                .cloned()
                .ok_or_else(|| Resp::err("CEO_TARGET_NAME_REQUIRED"))?
        };
        let from = format!("{}@{}", name, source_project);
        if let Some(id) = in_reply_to.as_deref() {
            // Resolve global authority before taking either project-state lock.
            let destination_is_ceo = self.ceo_grant().is_ok_and(|grant| {
                self.ceo_holder(&grant)
                    .is_ok_and(|(holder, id)| Arc::ptr_eq(&holder, target) && id == to)
            });
            let identical_reply_exists = {
                let state = target.state.lock().unwrap();
                state.msgs.values().any(|message| {
                    message.in_reply_to.as_deref() == Some(id)
                        && message.from == from
                        && message.to == to
                        && message.subject.as_deref() == Some(subject.as_str())
                        && message.body == body
                })
            };
            let state = source.state.lock().unwrap();
            let original = state
                .msgs
                .get(id)
                .ok_or_else(|| Resp::err("CEO_REPLY_ORIGINAL_NOT_FOUND"))?;
            let recipient = original.to == actor
                || state
                    .reply_obligations
                    .get(id)
                    .is_some_and(|o| o.recipient_name.as_deref() == Some(name));
            let expected_from = format!("{}@{}", target_name, target_project);
            let from_ceo = state.reply_obligations.get(id).is_some_and(|o| o.from_ceo);
            let sender_matches = if from_ceo {
                destination_is_ceo
            } else {
                original.from == expected_from
            };
            if !recipient || (!sender_matches && !identical_reply_exists) {
                return Err(Resp::err("CEO_REPLY_SCOPE_OR_RECIPIENT_REJECTED"));
            }
        }
        let correlation = in_reply_to.as_ref().or(handshake_id.as_ref());
        let source_thread = {
            let state = source.state.lock().unwrap();
            state
                .workers
                .get(actor)
                .and_then(selected_transport_for_worker)
                .and_then(|transport| notification_target(&transport).map(str::to_owned))
        };
        let (mid, subscription, task_id, deduplicated) = {
            let mut state = target.state.lock().unwrap();
            if state.admission_frozen() {
                return Err(Resp::err("MIGRATION_ADMISSION_FROZEN"));
            }
            if state
                .name_occupancy(&target_project, &target_name)
                .is_none_or(|o| o.worker_id != to)
            {
                return Err(Resp::err("CEO_TARGET_NAME_CHANGED"));
            }
            if let Some((new_task, _)) = &task {
                if state.tasks.contains_key(&new_task.id) {
                    return Err(Resp::err("BOARD_TASK_ALREADY_EXISTS"));
                }
                if current_master_holder(target, &state)
                    .map_err(Resp::err)?
                    .as_deref()
                    != Some(to)
                {
                    return Err(Resp::err("CEO_TARGET_MASTER_CHANGED"));
                }
            }
            let duplicate = correlation.and_then(|id| {
                state
                    .msgs
                    .values()
                    .find(|m| {
                        m.in_reply_to.as_ref() == Some(id)
                            && m.from == from
                            && m.to == to
                            && (handshake_id.is_some()
                                || (m.subject.as_deref() == Some(subject.as_str())
                                    && m.body == body))
                    })
                    .map(|m| m.id.clone())
            });
            if let Some(mid) = duplicate {
                (mid, None, None, true)
            } else {
                let mid = gen_msg_id();
                let message = Message {
                    id: mid.clone(),
                    from: from.clone(),
                    to: to.into(),
                    mtype: "notify".into(),
                    subject: Some(subject),
                    body,
                    in_reply_to: correlation.cloned(),
                    created_ms: now_ms(),
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                };
                let subscription = state
                    .matching_subscription(to, "direct-message", None, now_ms())
                    .cloned();
                let mut events = Vec::new();
                let task_id = if let Some((task, details)) = task {
                    let id = task.id.clone();
                    let occupancy = state
                        .name_occupancy(&target_project, &target_name)
                        .cloned()
                        .ok_or_else(|| Resp::err("CEO_TARGET_NAME_CHANGED"))?;
                    events.push(Event::TaskCreated { task });
                    events.push(Event::BoardDetailsChanged {
                        task_id: id.clone(),
                        details,
                    });
                    events.push(Event::TaskAssignmentUpdated {
                        assignment: name_state::TaskAssignment {
                            task_id: id.clone(),
                            name: target_name.clone(),
                            session_id: occupancy.session_id,
                            assignment_revision: 1,
                            reflow: None,
                            updated_ms: now_ms(),
                        },
                    });
                    Some(id)
                } else {
                    None
                };
                events.push(Event::Sent { msg: message });
                events.push(Event::MailboxNameAddressed {
                    message_id: mid.clone(),
                    address: name_mailbox::MailboxNameAddress {
                        project_scope: target_project.clone(),
                        name: target_name.clone(),
                    },
                });
                if let Some(policy) = policy {
                    let mut obligation = reply_obligation::build_obligation(
                        &state,
                        mid.clone(),
                        target_project.clone(),
                        actor.into(),
                        to.into(),
                        Some(target_name.clone()),
                        policy,
                    );
                    obligation.from_name = Some(name.into());
                    obligation.from_project = Some(source_project.clone());
                    obligation.from_ceo = true;
                    events.push(Event::ReplyObligationSet { obligation });
                }
                events.push(Event::DeliveryMode {
                    msg_id: mid.clone(),
                    mode: "explicit-notification".into(),
                    source_thread_id: source_thread,
                });
                if let Some(sub) = &subscription {
                    events.push(Event::WakeBound {
                        message_id: mid.clone(),
                        subscription_id: sub.id.clone(),
                    });
                }
                target
                    .commit_locked_checked(&mut state, &events)
                    .map_err(|e| Resp::err(format!("CEO_DELIVERY_DURABILITY_FAILED: {e}")))?;
                (mid, subscription, task_id, false)
            }
        };
        if let Some(original) = in_reply_to {
            let mut state = source.state.lock().unwrap();
            if state.reply_obligations.contains_key(&original) {
                if let Err(error) = source.commit_locked_checked(
                    &mut state,
                    &[Event::ReplyObligationCleared {
                        message_id: original,
                    }],
                ) {
                    let mut response = Resp::data(
                        json!({"msg_id":mid,"durable":true,"reply_marker_cleared":false}),
                    );
                    response.ok = false;
                    response.error = Some(format!(
                        "CEO_REPLY_MARKER_COMMIT_FAILED: reply is durable; do not resend: {error}"
                    ));
                    return Err(response);
                }
            }
        }
        let notification = if deduplicated {
            NotificationAttempt::NotAttempted("identical reply already durable".into())
        } else if let Some(sub) = &subscription {
            attempt_notification_detailed_with_at(target, &mid, &sub.id, now_ms())
        } else {
            NotificationAttempt::NotAttempted("no notification subscription".into())
        };
        let method = {
            let state = target.state.lock().unwrap();
            notification_method_for_worker(&state, to)
        };
        Ok(notification_send_response(
            json!({"msg_id":mid,"task_id":task_id,"cross_project":true,"durable":true,"deduplicated":deduplicated,"target_master":to}),
            subscription.is_some(),
            method,
            &notification,
        ))
    }

    /// Resolve the official central Collab Skill and user preference owner from
    /// the current environment. This never guesses a task home and never copies
    /// private preference content.
    fn ceo_workspace_guidance_sources(&self) -> Result<(PathBuf, PathBuf), String> {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            "HOME is unavailable; cannot resolve USER.md or the central Collab Skill".to_owned()
        })?;
        let home = PathBuf::from(home);
        Ok((
            home.join(".codex").join("USER.md"),
            home.join(".agents")
                .join("skills")
                .join("collab")
                .join("SKILL.md"),
        ))
    }

    /// Attach workspace guidance to an already-authorized Promote result. The
    /// grant is durable first; a guidance failure is partial success, never a
    /// reason to rewrite the source registration or fall back to another file.
    fn attach_ceo_workspace_guidance(&self, status: &mut Resp, grant: &CeoGrant) {
        let guidance = self
            .ceo_workspace_guidance_sources()
            .and_then(|(preferences, skill)| {
                crate::ceo_workspace_guidance::ensure_ceo_workspace_guidance(
                    Path::new(&grant.workspace),
                    &preferences,
                    &skill,
                )
                .map_err(|error| error.to_string())
            });
        if !status.data.is_object() {
            status.data = json!({});
        }
        let object = status
            .data
            .as_object_mut()
            .expect("CEO status data was just ensured to be an object");
        match guidance {
            Ok(receipt) => {
                object.insert(
                    "workspace_guidance".into(),
                    json!({
                        "ok": true,
                        "path": receipt.path,
                        "action": receipt.action.as_str(),
                    }),
                );
            }
            Err(error) => {
                object.insert(
                    "workspace_guidance".into(),
                    json!({"ok": false, "grant_durable": true, "error": error}),
                );
                status.ok = false;
                status.error = Some(format!(
                    "CEO_GRANT_RECORDED_BUT_WORKSPACE_GUIDANCE_FAILED: {error}"
                ));
            }
        }
    }

    /// Record trimmed project progress into the host canonical report state.
    /// Exact business content is the dedup key. Recording never clears a reply
    /// obligation and never claims a user report.
    fn ceo_record_progress(
        &self,
        source: &Arc<Server>,
        subject: String,
        body: String,
        urgent: bool,
    ) -> Result<serde_json::Value, Resp> {
        let content = crate::ceo_report_state::CeoProgressContent::new(subject, body);
        if content.subject.is_empty() || content.body.is_empty() {
            return Err(Resp::err("CEO_MESSAGE_CONTENT_REQUIRED"));
        }
        let project = source.root.to_string_lossy().into_owned();
        let mut state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(Resp::err(format!(
                "CEO_AUTHORITY_STORE_UNAVAILABLE: {error}"
            )));
        }
        // Probe the helper dedup on a clone so the response can state truthfully
        // whether this call created a meaningful revision.
        let mut probe = state.ceo_report_state.clone();
        let recorded = probe.record_progress(project.clone(), content.clone(), urgent);
        if recorded {
            self.host
                .commit_locked_checked(
                    &mut state,
                    &[Event::CeoProgressRecorded {
                        project: project.clone(),
                        content,
                        urgent,
                    }],
                )
                .map_err(|error| Resp::err(format!("CEO_PROGRESS_DURABILITY_FAILED: {error}")))?;
        }
        let revision = state
            .ceo_report_state
            .projects
            .get(&project)
            .map(|progress| progress.revision);
        Ok(json!({
            "project": project,
            "recorded": recorded,
            "deduplicated": !recorded,
            "urgent": urgent,
            "revision": revision,
        }))
    }

    /// Unsolicited periodic progress has no immediate delivery. The scheduler
    /// (added in the next pass) owns any notification, so this answer only
    /// reports the durable recording outcome.
    fn ceo_record_unsolicited_progress(
        &self,
        source: &Arc<Server>,
        subject: String,
        body: String,
        urgent: bool,
    ) -> Result<Resp, Resp> {
        let mut progress = self.ceo_record_progress(source, subject, body, urgent)?;
        if let Some(object) = progress.as_object_mut() {
            object.insert("notified".into(), json!(false));
            object.insert("reported".into(), json!(false));
        }
        Ok(Resp::data(progress))
    }

    /// Add canonical progress to a solicited reply result. A durable reply whose
    /// source obligation clear failed is still a valid reply, so progress is
    /// recorded and the partial failure stays visible. Hard delivery failures
    /// record nothing.
    fn attach_solicited_progress(
        &self,
        delivered: Result<Resp, Resp>,
        source: &Arc<Server>,
        subject: String,
        body: String,
        urgent: bool,
    ) -> Result<Resp, Resp> {
        match delivered {
            Ok(mut response) => {
                // The reply is already durable. If progress cannot be recorded,
                // keep the real msg_id visible and forbid an uncertain resend.
                match self.ceo_record_progress(source, subject, body, urgent) {
                    Ok(progress) => merge_ceo_json_object(&mut response.data, progress),
                    Err(progress_error) => {
                        let error = progress_error
                            .error
                            .unwrap_or_else(|| "CEO_PROGRESS_RECORD_FAILED".to_owned());
                        if let Some(object) = response.data.as_object_mut() {
                            object.insert("progress_recorded".into(), json!(false));
                            object.insert("progress_error".into(), json!(error));
                        }
                        response.ok = false;
                        response.error = Some(format!(
                            "CEO_REPLY_DURABLE_PROGRESS_FAILED: reply is durable; do not resend: {error}"
                        ));
                    }
                }
                Ok(response)
            }
            Err(mut response) => {
                let durable = matches!(
                    response.data.get("durable"),
                    Some(serde_json::Value::Bool(true))
                );
                if durable {
                    match self.ceo_record_progress(source, subject, body, urgent) {
                        Ok(progress) => merge_ceo_json_object(&mut response.data, progress),
                        Err(progress_error) => {
                            if let Some(object) = response.data.as_object_mut() {
                                object.insert("progress_recorded".into(), json!(false));
                                object.insert(
                                    "progress_error".into(),
                                    progress_error
                                        .error
                                        .map_or(serde_json::Value::Null, serde_json::Value::String),
                                );
                            }
                        }
                    }
                }
                Err(response)
            }
        }
    }

    /// Acknowledge one visible CEO batch. Only the current CEO reaches this
    /// handler; an unknown or already-consumed batch is rejected before any
    /// event is appended.
    fn ceo_report_ack(&self, batch_id: String) -> Result<Resp, Resp> {
        if batch_id.trim().is_empty() {
            return Err(Resp::err("CEO_REPORT_BATCH_REQUIRED"));
        }
        let reported_ms = now_ms().max(0) as u64;
        let mut state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(Resp::err(format!(
                "CEO_AUTHORITY_STORE_UNAVAILABLE: {error}"
            )));
        }
        if !state.ceo_report_state.batches.contains_key(&batch_id) {
            return Err(Resp::err(
                "CEO_REPORT_BATCH_UNKNOWN: no unacknowledged batch has this id",
            ));
        }
        self.host
            .commit_locked_checked(
                &mut state,
                &[Event::CeoReportBatchAcknowledged {
                    batch_id: batch_id.clone(),
                    reported_ms,
                }],
            )
            .map_err(|error| Resp::err(format!("CEO_REPORT_ACK_DURABILITY_FAILED: {error}")))?;
        Ok(Resp::data(json!({
            "batch_id": batch_id,
            "reported_ms": reported_ms,
            "last_actual_report_ms": state.ceo_report_state.last_actual_report_ms(),
            "pending_revision_count": state.ceo_report_state.pending_revisions().len(),
            "remaining_batch_count": state.ceo_report_state.batch_count(),
        })))
    }

    /// Select and prepare one report batch under a single host lock. Ordinary
    /// suppression narrows the batch to explicit typed urgent projects only.
    /// Reading never advances a reported watermark.
    fn ceo_prepare_report_batch(&self, project: Option<&str>) -> Result<serde_json::Value, Resp> {
        let policy = self
            .host
            .config
            .ceo_report_policy()
            .map_err(|error| Resp::err(format!("CEO_REPORT_POLICY_INVALID: {error}")))?;
        let now = now_ms().max(0) as u64;
        let mut state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(Resp::err(format!(
                "CEO_AUTHORITY_STORE_UNAVAILABLE: {error}"
            )));
        }
        let last = state.ceo_report_state.last_actual_report_ms();
        let decision = policy.evaluate(now, last, false);
        let pending: Vec<_> = state
            .ceo_report_state
            .pending_revisions()
            .into_iter()
            .filter(|revision| project.is_none_or(|wanted| revision.project == wanted))
            .collect();
        let total = pending.len();
        let selected: Vec<_> = if decision.allowed {
            pending
        } else {
            pending
                .into_iter()
                .filter(|revision| revision.urgent)
                .collect()
        };
        if selected.is_empty() {
            return Ok(json!({
                "batch_id": serde_json::Value::Null,
                "coverage": {},
                "projects": [],
                "last_actual_report_ms": last,
                "suppressed": !decision.allowed,
                "suppression": ceo_suppression_label(decision.suppression),
                "next_eligible_ms": decision.next_eligible_ms,
                "excluded_project_count": total,
            }));
        }
        let coverage: std::collections::BTreeMap<String, u64> = selected
            .iter()
            .map(|revision| (revision.project.clone(), revision.revision))
            .collect();
        let urgent = !decision.allowed;
        // The helper owns batch dedup. Ask it for the actual batch first so a
        // deduplicated coverage reports the stored ID, then append the event
        // only when this call created a new batch.
        let mut probe = state.ceo_report_state.clone();
        let prepared = probe
            .prepare_batch(gen_msg_id(), coverage.clone(), urgent)
            .map_err(|error| Resp::err(format!("CEO_REPORT_BATCH_PREPARE_FAILED: {error}")))?;
        let batch_id = prepared.batch_id;
        if !state.ceo_report_state.batches.contains_key(&batch_id) {
            self.host
                .commit_locked_checked(
                    &mut state,
                    &[Event::CeoReportBatchPrepared {
                        batch_id: batch_id.clone(),
                        coverage: coverage.clone(),
                        urgent,
                    }],
                )
                .map_err(|error| {
                    Resp::err(format!("CEO_REPORT_BATCH_DURABILITY_FAILED: {error}"))
                })?;
        }
        let included = selected
            .iter()
            .map(|revision| {
                json!({
                    "project": revision.project.clone(),
                    "revision": revision.revision,
                    "subject": revision.content.subject.clone(),
                    "body": revision.content.body.clone(),
                    "urgent": revision.urgent,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "batch_id": batch_id,
            "coverage": coverage,
            "projects": included,
            "last_actual_report_ms": state.ceo_report_state.last_actual_report_ms(),
            "suppressed": !decision.allowed,
            "suppression": ceo_suppression_label(decision.suppression),
            "next_eligible_ms": decision.next_eligible_ms,
            "excluded_project_count": total - selected.len(),
        }))
    }

    fn ceo_progress(&self, project: Option<String>) -> Result<Resp, Resp> {
        let (roots, project_filter) = if let Some(project) = project {
            let root = std::fs::canonicalize(project)
                .map_err(|e| Resp::err(format!("CEO_PROJECT_INVALID: {e}")))?;
            (vec![root.clone()], Some(root))
        } else {
            let roots = self
                .routes
                .lock()
                .unwrap()
                .values()
                .map(|r| r.root.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            (roots, None)
        };
        let mut projects = Vec::new();
        for root in &roots {
            let runtimes = self.ceo_project_runtimes(&root.to_string_lossy(), None)?;
            if runtimes.is_empty() {
                projects.push(json!({
                    "project":root,
                    "registered":false,
                    "master":null,
                    "workspace": unknown_workspace_view(Some(
                        root.to_string_lossy().into_owned(),
                    )),
                }));
                continue;
            }
            for runtime in runtimes {
                let state = runtime.state.lock().unwrap();
                let master = current_master_holder(&runtime, &state).map_err(Resp::err)?;
                let scope = server_route_scope(&runtime, &state).map_err(Resp::err)?;
                let workspace =
                    workspace_idle_view(&runtime, &state, master.as_deref(), now_ms());
                let mut counts = std::collections::BTreeMap::<String, usize>::new();
                let mut tasks = Vec::new();
                for task in state.tasks.values().filter(|t| {
                    state
                        .board_details
                        .get(&t.id)
                        .is_some_and(|d| d.public_visibility)
                }) {
                    if let Some(view) = board_task_view_with_master(
                        &state,
                        task,
                        None,
                        master.as_deref(),
                        scope.as_ref(),
                    ) {
                        *counts.entry(view.status.clone()).or_default() += 1;
                        tasks.push(serde_json::to_value(view).expect("board view serializes"));
                    }
                }
                tasks.sort_by_key(|t| t["id"].as_str().unwrap_or_default().to_owned());
                let pending = state
                    .reply_obligations
                    .values()
                    .filter(|o| o.from_ceo)
                    .cloned()
                    .collect::<Vec<_>>();
                projects.push(json!({"project":root,"app_scope_id":scope.as_ref().map(|s|s.app_scope_id.as_str()),"registered":true,"master":master,"workspace":workspace,"tasks":tasks,"task_counts":counts,"pending_replies":pending}));
            }
        }
        let operations = self
            .host
            .state
            .lock()
            .unwrap()
            .ceo_bootstraps
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let filter = project_filter
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned());
        let report = self.ceo_prepare_report_batch(filter.as_deref())?;
        Ok(Resp::data(json!({
            "projects": projects,
            "bootstrap_operations": operations,
            "report": report,
        })))
    }
}

/// Merge the object fields of `extra` into `target`. Non-object inputs are left
/// untouched; both producers here always build JSON objects.
fn merge_ceo_json_object(target: &mut serde_json::Value, extra: serde_json::Value) {
    if let (Some(target), serde_json::Value::Object(extra)) = (target.as_object_mut(), extra) {
        for (key, value) in extra {
            target.insert(key, value);
        }
    }
}

/// Stable wire label for a time-policy outcome.
fn ceo_suppression_label(
    suppression: crate::ceo_reporting_policy::CeoReportSuppression,
) -> &'static str {
    use crate::ceo_reporting_policy::CeoReportSuppression;
    match suppression {
        CeoReportSuppression::Allowed => "allowed",
        CeoReportSuppression::Cooldown => "cooldown",
        CeoReportSuppression::QuietHours => "quiet-hours",
    }
}
