use crate::server::binding_slot::{
    BindingControlSet, IdentityBindingKey, PendingEffect, PendingEffectKind, RoleAction,
    RoleAuthorization, RoleEffect,
};

include!("anchor_rename.rs");
include!("communication.rs");
include!("retirement.rs");

impl ProjectRuntimeManager {
    pub(super) fn identity_context(
        &self,
        context: ProjectContext,
        request: IdentityContextRequest,
    ) -> (Arc<Server>, Resp) {
        self.mapped_context(context, request)
    }

    fn mapped_context(
        &self,
        context: ProjectContext,
        mut request: IdentityContextRequest,
    ) -> (Arc<Server>, Resp) {
        if request.action != "context" && !request.query {
            return (
                self.host.clone(),
                Resp::err("IDENTITY_ACTION_UNSUPPORTED: identity requests use the current map"),
            );
        }
        let observed = self.observe_identity_facts_with_pane(&context, &request.facts);
        let (facts, pane) = match observed {
            Ok(observed) => observed,
            Err(error) => return (self.host.clone(), Resp::err(format!("{error:#}"))),
        };
        request.facts = facts.clone();
        match self.requested_name_scope_requirement(&context, &request) {
            Ok(Some(requirement)) => return (self.host.clone(), requirement),
            Ok(None) => {}
            Err(error) => return (self.host.clone(), Resp::err(error)),
        }
        if facts.dsh_session_id.is_some() && facts.dsh.is_none() {
            let mapped = self.binding_control_store().load().and_then(|map| {
                map.lookup(
                    &context.project_scope,
                    &context.app_scope_id,
                    &facts,
                    request.invocation_cwd.as_deref(),
                )
                .map(|slot| slot.is_some())
            });
            match mapped {
                Ok(false) => {
                    let data = json!({"snapshot":{"requires_identity_update":{"required_fields":["dsh"],
                        "field_descriptions":{"dsh":"Current gateway endpoint, runtime, agent, session and cwd"},
                        "action":"collab context --provide '<current dsh binding>'"}}});
                    return (
                        self.host.clone(),
                        typed_operation_resp(missing_facts_result(&request, &data)),
                    );
                }
                Err(error) => return (self.host.clone(), Resp::err(error)),
                Ok(true) => {}
            }
        }
        if request.query {
            if let Some(retirement) = self.query_human_retirement(&context, &request) {
                return retirement;
            }
            return self.current_mapped_context(&context, &request);
        }
        if let Some(approval) = request
            .grant_approval
            .as_ref()
            .or(request.approval.as_ref())
        {
            if approval
                .get("project_scope")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|scope| scope != context.project_scope.as_str())
                || approval
                    .get("app_scope_id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|scope| scope != context.app_scope_id.as_str())
            {
                return (
                    self.host.clone(),
                    Resp::err("IDENTITY_APPROVAL_SCOPE_MISMATCH"),
                );
            }
            if !matches!(
                approval["action"].as_str(),
                Some("restore_master" | "restore_peer" | "restore_name" | "retire_name")
            ) {
                return (
                    self.host.clone(),
                    Resp::err(
                        "IDENTITY_APPROVAL_ACTION_UNSUPPORTED: use current name or role intent",
                    ),
                );
            }
            let retire_name = approval["action"].as_str() == Some("retire_name");
            if !approval
                .get("authorization")
                .or(approval.get("approval"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !text.trim().is_empty() && !text.chars().any(char::is_control))
            {
                return (
                    self.host.clone(),
                    Resp::err("IDENTITY_AUTHORIZATION_REQUIRED"),
                );
            }
            // Explicit human retirement is a release action with its own
            // dedicated dispatch. It must not fall through to the ordinary
            // register/allocate path.
            if retire_name {
                return self.human_retire_name(&context, &request, approval);
            }
        }
        // Parse authorized target before any registration can publish a name.
        let raw_authorization = request.grant_approval.as_ref().or(request.approval.as_ref());
        let mut intent = match name_resolution::parse_name_intent(raw_authorization) {
            Ok(intent) => intent,
            Err(error) => return (self.host.clone(), Resp::err(error)),
        };
        let allow_explicit_rex_rebind = matches!(request
            .grant_approval
            .as_ref()
            .or(request.approval.as_ref())
            .and_then(|approval| approval["action"].as_str()),
            Some("restore_name" | "restore_master"));
        let rex_recovery = if facts.rex.is_some() && !allow_explicit_rex_rebind {
            match self.observe_rex_recovery(
                &context,
                &facts,
                request.invocation_cwd.as_deref(),
            ) {
                Ok(observation) => Some(observation),
                Err(error) => {
                    return (self.host.clone(), Resp::err(format!("{error:#}")));
                }
            }
        } else {
            None
        };
        let mut facts = facts;
        facts.caller_pid = None;
        facts.pane_hint = None;
        request.facts = facts.clone();
        if !rex_recovery.as_ref().is_some_and(|recovery|
            matches!(recovery.admission, RexRecoveryAdmission::DisplacedExecution(_))) {
            let _identity = self.identity_gate.lock().unwrap();
            if let Err(error) = self.ensure_current_worker_for_context(
                &context,
                &facts,
                request.invocation_cwd.as_deref(),
                allow_explicit_rex_rebind,
            ) {
                return (self.host.clone(), Resp::err(error));
            }
        }
        // DSH convergence owns the gate when it changes the current binding.
        if let Err(error) = self.reconcile_current_dsh_session(&context, &facts) {
            return (self.host.clone(), Resp::err(format!("{error:#}")));
        }
        let _identity = self.identity_gate.lock().unwrap();
        let correlation = if request.operation_id.is_empty() {
            format!("context-{}-{}", std::process::id(), now_ms())
        } else {
            request.operation_id.clone()
        };
        let outer = RegisterOuterBinding::new()
            .with_identity_facts(facts.clone())
            .with_name_tmux_session_name(facts.tmux_session_name.clone())
            .with_name_pane_anchor(
                pane.clone()
                    .zip(request.invocation_cwd.clone())
                    .map(|(pane, cwd)| name_state::NamePaneAnchor { pane, cwd }),
            )
            .with_rex_recovery(rex_recovery)
            .with_name_intent(intent.clone())
            .with_role_intent(&request);
        let reconciled = self.reconcile_identity_context(
            context.clone(),
            facts.clone(),
            Some(outer),
            true,
            allow_explicit_rex_rebind,
            request.invocation_cwd.as_deref(),
        );
        let (runtime, mut response) = match reconciled {
            Ok(result) => result,
            Err(error) => return (self.host.clone(), Resp::err(format!("{error:#}"))),
        };
        if !response.ok {
            return (runtime, response);
        }
        if response.data["identity_receipt"].is_null() {
            return (
                runtime,
                typed_operation_resp(missing_facts_result(&request, &response.data)),
            );
        }
        let identity: identity::Identity =
            match serde_json::from_value(response.data["identity_receipt"].clone()) {
                Ok(identity) => identity,
                Err(error) => {
                    return (
                        runtime,
                        Resp::err(format!("IDENTITY_RECEIPT_INVALID: {error}")),
                    )
                }
            };
        if raw_authorization.is_some_and(|value| value["action"] == "restore_peer") {
            let name = match self.binding_control_store().load().and_then(|map| {
                map.ordinary_name(
                    &context.project_scope,
                    &context.app_scope_id,
                    &identity.worker_id,
                )
            }) {
                Ok(name) => name,
                Err(error) => return (runtime, Resp::err(error)),
            };
            intent = Some(name_resolution::NameIntent {
                requested_name: name,
                user_authorized: true,
                moves_existing_names: false,
                replace_confirmed: true,
            });
        }
        let authorization = raw_authorization
            .and_then(|raw| raw.get("authorization").or_else(|| raw.get("approval")))
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(|text| RoleAuthorization::User {
                authorization: text.to_owned(),
                granted_by: raw_authorization
                    .and_then(|raw| raw["decided_by"].as_str())
                    .unwrap_or("user")
                    .to_owned(),
            });
        let name_result = self.resolve_context_name_control_first(
            &runtime,
            &context,
            &identity.worker_id,
            &correlation,
            intent,
            authorization,
            pane.zip(request.invocation_cwd.clone())
                .map(|(pane, cwd)| name_state::NamePaneAnchor { pane, cwd }),
            Some(&facts),
        );
        let name_result = match name_result {
            Ok(result) => result,
            Err(error) => return (runtime, Resp::err(error)),
        };
        let physical = name_result["physical_anchor"].clone();
        let physical_incomplete = physical["status"] == "incomplete";
        drop(_identity);
        let communication = match self.mapped_communication(&runtime, &context,
            &identity.worker_id, &identity.token,
            request.invocation == "manual_communication_restore", false) {
            Ok(communication) => communication,
            Err(error) => return (runtime, Resp::err(error)),
        };
        let mut snapshot =
            context_projection(&runtime, identity.worker_id.clone(), identity.token.clone());
        if !snapshot.ok {
            return (runtime, snapshot);
        }
        response.data["snapshot"] = snapshot.data;
        if request.operation_id.is_empty() && communication.is_none() && !physical_incomplete {
            return (runtime, response);
        }
        let outcome = if physical_incomplete { "incomplete" } else {
            communication.as_ref().map(communication_operation_outcome).unwrap_or("completed")
        };
        let mut result =
            json!({"result":{
                "operation_id":request.operation_id,"action":"context","invocation":request.invocation,
                "outcome":outcome,"phase":"context_complete","committed_phases":[],"failed_phase":null,
                "requires":{"kind":null,"fields":[],"sources":{},"approval":null,"repair_invocation":null},
                "snapshot":response.data["snapshot"],"owner_readback":{"physical_anchor":physical}
            }});
        if physical_incomplete {
            result["result"]["phase"] = json!("physical_anchor");
            result["result"]["failed_phase"] = json!("physical_anchor");
            result["result"]["committed_phases"] = json!(["current", "business_effects"]);
            result["result"]["requires"]["kind"] = json!("physical_anchor");
            result["result"]["requires"]["repair_invocation"] = json!("collab context");
        }
        let error = physical_incomplete.then(|| physical["error"].as_str()
            .unwrap_or("PHYSICAL_ANCHOR_INCOMPLETE").to_owned());
        if let Some(communication) = communication { result["result"]["owner_readback"]["communication"] = communication; }
        (
            runtime,
            Resp { ok: outcome == "completed", error, data: result },
        )
    }

    /// A uniquely held target name keeps its existing app partition. Resolve
    /// that partition before registration so a wrong `--app-scope` cannot
    /// allocate an ordinary name or mutate the map first.
    fn requested_name_scope_requirement(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> Result<Option<Resp>, String> {
        let Some(raw) = request
            .grant_approval
            .as_ref()
            .or(request.approval.as_ref())
        else {
            return Ok(None);
        };
        let Some(intent) = name_resolution::parse_name_intent(Some(raw))? else {
            return Ok(None);
        };
        let requested_name = &intent.requested_name;
        let control = self.binding_control_store().load()?;
        let targets = control.project_currents_for_name(&context.project_scope, requested_name);
        if targets.is_empty()
            || targets.iter().any(|slot| slot.identity_key.app_scope_id == context.app_scope_id)
        {
            return Ok(None);
        }
        let target_apps = targets
            .iter()
            .map(|slot| slot.identity_key.app_scope_id.as_str().to_owned())
            .collect::<Vec<_>>();
        let unique_target = (targets.len() == 1).then(|| targets[0]);
        let target_app = unique_target.map(|slot| slot.identity_key.app_scope_id.as_str());
        let repair_invocation = match target_app {
            Some(target_app) => format!(
                "collab context --project <project> --app-scope {target_app} --approve-grant <existing-approval-json>"
            ),
            None => format!(
                "collab context --project <project> --app-scope <one of: {}> --approve-grant <existing-approval-json>",
                target_apps.join(", ")
            ),
        };
        let sources = json!({
            "requested_name": requested_name,
            "target_app_scope_id": target_app,
            "target_app_scope_ids": target_apps,
            "target_key": unique_target.map(|slot| &slot.identity_key),
            "target_keys": targets.iter().map(|slot| &slot.identity_key).collect::<Vec<_>>(),
        }).as_object().unwrap().clone();
        Ok(Some(typed_operation_resp(crate::proto::ContextOperationResult {
            operation_id: request.operation_id.clone(),
            invocation: request.invocation.clone(),
            action: request.action.clone(),
            phase: Some("target_scope_resolution".into()),
            outcome: "scope_required".into(),
            committed_phases: Vec::new(),
            failed_phase: None,
            requires: crate::proto::ContextOperationRequires {
                kind: Some("app_scope".into()),
                fields: vec!["app_scope_id".into()],
                sources,
                repair_invocation: Some(repair_invocation),
                ..Default::default()
            },
            snapshot: Some(json!({"registered": false, "identity": null})),
            owner_readback: Default::default(),
        })))
    }

    fn current_mapped_context(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> (Arc<Server>, Resp) {
        match self.pending_registration_context(context, request) {
            Ok(Some(result)) => return result,
            Ok(None) => {}
            Err(error) => return (self.host.clone(), Resp::err(error)),
        }
        let identity = match self.current_session_identity_at_cwd(
            context,
            &request.facts,
            request.invocation_cwd.as_deref(),
            false,
        ) {
            Ok(Some(identity)) => identity,
            Ok(None) => {
                return (
                    self.host.clone(),
                    Resp::err("IDENTITY_MAP_NOT_FOUND: current caller has no mapping"),
                )
            }
            Err(error) => return (self.host.clone(), Resp::err(format!("{error:#}"))),
        };
        let runtime = self
            .routes
            .lock()
            .unwrap()
            .get(&Self::route_key(context))
            .and_then(|route| route.runtime.clone());
        let Some(runtime) = runtime else {
            return (
                self.host.clone(),
                Resp::err("IDENTITY_CONTEXT_READBACK_UNAVAILABLE: mapped owner is not loaded"),
            );
        };
        let communication = match self.mapped_communication(&runtime, context,
            &identity.worker_id, &identity.token, false, true) {
            Ok(communication) => communication,
            Err(error) => return (runtime, Resp::err(error)),
        };
        let mut snapshot = context_projection(&runtime, identity.worker_id.clone(), identity.token);
        if !snapshot.ok {
            return (runtime, snapshot);
        }
        let communication_outcome = communication.as_ref().map(communication_operation_outcome).unwrap_or("completed");
        let physical = snapshot.data["physical_anchor"].clone();
        let outcome = if physical["status"] == "incomplete" { "incomplete" } else { communication_outcome };
        let mut result = json!({"result":{"operation_id":request.operation_id,"action":"context","invocation":"query",
            "outcome":outcome,"phase":null,"committed_phases":[],"failed_phase":null,
            "requires":{"kind":null,"fields":[],"sources":{},"approval":null,"repair_invocation":null},"snapshot":snapshot.data,
            "owner_readback":{"physical_anchor":physical}}});
        if let Some(communication) = communication { result["result"]["owner_readback"]["communication"] = communication; }
        (
            runtime,
            Resp { ok: communication_outcome == "completed", error: None, data: result },
        )
    }

    /// A published current map can precede its Register business commit. Project
    /// only the map-owned facts and the exact pending effect; never read a
    /// credential, reconstruct a worker from State history, or consume work.
    fn pending_registration_context(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> Result<Option<(Arc<Server>, Resp)>, String> {
        let control = self.binding_control_store().load()?;
        let Some(slot) = control.lookup(
            &context.project_scope,
            &context.app_scope_id,
            &request.facts,
            request.invocation_cwd.as_deref(),
        )? else {
            return Ok(None);
        };
        let value = slot
            .active_binding()
            .expect("lookup returns active current");
        if request
            .facts
            .dsh
            .as_ref()
            .is_some_and(|candidate| {
                !identity::dsh_transport_matches(candidate, &value.transport)
            })
        {
            return Err(
                "IDENTITY_RESTORE_CONFLICT: DSH communication API differs from the current mapped binding"
                    .into(),
            );
        }
        let Some(session_id) = value.binding.session_id.as_ref() else {
            return Ok(None);
        };
        let worker_id = value.binding.agent_id.as_str();
        let runtime = self.select_runtime(context)?;
        if runtime.state.lock().unwrap().workers.contains_key(worker_id) {
            return Ok(None);
        }
        let pending: Vec<_> = control
            .pending_effects()
            .filter(|effect| {
                effect.is_register()
                    && effect.project_scope == context.project_scope
                    && effect.app_scope_id == context.app_scope_id
                    && effect.name == slot.identity_key.name
                    && effect.target_worker_id == worker_id
                    && effect.target_session_id == session_id.as_str()
            })
            .collect();
        if pending.is_empty() {
            return Ok(None);
        }
        let pending_views: Vec<_> = pending
            .iter()
            .map(|effect| {
                json!({
                    "effect_id": effect.effect_id,
                    "operation_id": effect.operation_id,
                    "kind": serde_json::to_value(&effect.kind)
                        .unwrap_or(serde_json::Value::Null),
                    "name": effect.name,
                    "target_worker_id": effect.target_worker_id,
                    "target_session_id": effect.target_session_id,
                    "created_ms": effect.created_ms,
                })
            })
            .collect();
        let name = slot.identity_key.name.clone();
        let snapshot = json!({
            "schema_version": 1,
            "registration": {
                "status": "pending",
                "project_root": runtime.root,
                "worker_id": worker_id,
                "cwd": value.invocation_cwd,
            },
            "name": name,
            "session_id": value.binding.session_id,
            "layout": tm_layout::projection(value),
            "physical_anchor": physical_anchor_projection(value, &name),
            "binding": value.binding,
            "registered": false,
            "project_root": runtime.root,
            "identity": {
                "worker_id": worker_id,
                "display_name": name,
                "kind": "peer",
                "role": "unknown",
                "transport": value.transport,
            },
            "work_visibility": "pending registration: role and work projections are unavailable until Register commits",
            "truth": "identity-map.json owns the published current; the Register effect is still pending in business State"
        });
        let result = json!({"result":{
            "operation_id":request.operation_id,
            "action":request.action,
            "invocation":"query",
            "outcome":"partial",
            "phase":"business_effects_pending",
            "committed_phases":["current"],
            "failed_phase":null,
            "requires":{
                "kind":"register_effect",
                "fields":[],
                "sources":{"effect_ids":pending.iter().map(|effect| effect.effect_id.clone()).collect::<Vec<_>>()},
                "approval":null,
                "repair_invocation":"collab context"
            },
            "snapshot":snapshot,
            "owner_readback":{"pending_effects":pending_views}
        }});
        Ok(Some((
            runtime,
            Resp {
                ok: true,
                error: None,
                data: result,
            },
        )))
    }

    pub(super) fn change_mapped_role(
        &self,
        runtime: &Arc<Server>,
        context: &ProjectContext,
        request: &Req,
    ) -> Resp {
        let (worker, token, approval, delegate) = match request {
            Req::MasterPromote {
                worker_id,
                token,
                approval,
            }
            | Req::MasterClear {
                worker_id,
                token,
                approval,
            } => (
                worker_id.as_str(),
                token.as_str(),
                Some(approval.as_str()),
                None,
            ),
            Req::MasterDelegate {
                worker_id,
                token,
                target_id,
                approval,
            } => (
                worker_id.as_str(),
                token.as_str(),
                approval.as_deref(),
                Some(target_id.as_str()),
            ),
            _ => return Resp::err("MASTER_CHANGE_INVALID"),
        };
        if let Err(error) = validate_request_context(runtime, request, Some(context)) {
            return Resp::err(error);
        }
        let map = match self.binding_control_store().load() {
            Ok(map) => map,
            Err(error) => return Resp::err(error),
        };
        let state = runtime.state.lock().unwrap();
        if let Err(error) = verify(&state, worker, token) {
            return error;
        }
        let route = RouteScope {
            project_scope_id: context.project_scope.clone(),
            app_scope_id: context.app_scope_id.clone(),
        };
        let previous = current_master_worker_id(&state, Some(&route));
        let (subject, target, authorization) = if let Some(target) = delegate {
            let Some(_grant) = current_master_grant(&state, Some(&route))
                .filter(|grant| grant.agent_id.as_str() == worker)
            else {
                return Resp::err(
                    "MASTER_DELEGATE_DENIED: caller must hold the current master grant",
                );
            };
            let Some(approval) = approval
                .filter(|text| !text.trim().is_empty() && !text.chars().any(char::is_control))
            else {
                return Resp::err("MASTER_AUTHORIZATION_REQUIRED: a new master requires explicit user approval");
            };
            (
                target.to_owned(),
                "Jenny".to_owned(),
                RoleAuthorization::User {
                    authorization: approval.into(),
                    granted_by: "user".into(),
                },
            )
        } else {
            let Some(approval) = approval
                .filter(|text| !text.trim().is_empty() && !text.chars().any(char::is_control))
            else {
                return Resp::err("master change requires valid explicit user approval");
            };
            let authorization = RoleAuthorization::User {
                authorization: approval.into(),
                granted_by: "user".into(),
            };
            if matches!(request, Req::MasterClear { .. }) {
                let Some(subject) = previous.clone() else {
                    return Resp::data(
                        json!({"master":null,"cleared":true,"previous_worker_id":null,"was_empty":true,"scope":scope_view(Some(&route))}),
                    );
                };
                let target = match map.ordinary_name(
                    &context.project_scope,
                    &context.app_scope_id,
                    &subject,
                ) {
                    Ok(name) => name,
                    Err(error) => return Resp::err(error),
                };
                (subject, target, authorization)
            } else {
                (worker.into(), "Jenny".into(), authorization)
            }
        };
        drop(state);
        let operation = format!("role-{}-{}", std::process::id(), now_ms());
        let result = self.resolve_context_name_control_first(
            runtime,
            context,
            &subject,
            &operation,
            Some(name_resolution::NameIntent {
                requested_name: target.clone(),
                user_authorized: true,
                moves_existing_names: false,
                replace_confirmed: true,
            }),
            Some(authorization),
            None,
            None,
        );
        match result {
            Ok(outcome) => {
                let control = match self.binding_control_store().load() {
                    Ok(control) => control, Err(error) => return Resp::err(error),
                };
                let state = runtime.state.lock().unwrap();
                let brief = match role_brief(runtime, &state, &control, &subject) {
                    Ok(brief) => brief, Err(error) => return Resp::err(error),
                };
                let mut response = json!({"master":current_master_worker_id(&state, Some(&route)),"scope":scope_view(Some(&route)),
                    "name_resolution":outcome,"mode":"user_approved_self_promotion","role_brief":brief});
                if matches!(request, Req::MasterClear { .. }) {
                    response["cleared"] = json!(true);
                    response["was_empty"] = json!(previous.is_none());
                    response["previous_worker_id"] = json!(previous);
                }
                if outcome["physical_anchor"]["status"] == "incomplete" {
                    response["outcome"] = json!("incomplete");
                    return Resp::err_data(
                        outcome["physical_anchor"]["error"].as_str()
                            .unwrap_or("PHYSICAL_ANCHOR_INCOMPLETE"),
                        response,
                    );
                }
                Resp::data(response)
            }
            Err(error) => Resp::err(error),
        }
    }

    fn resolve_context_name_control_first(
        &self,
        runtime: &Server,
        context: &ProjectContext,
        worker: &str,
        operation: &str,
        intent: Option<name_resolution::NameIntent>,
        authorization: Option<RoleAuthorization>,
        pane: Option<name_state::NamePaneAnchor>,
        facts: Option<&IdentityFacts>,
    ) -> Result<serde_json::Value, String> {
        let _register = self.register_gate.lock().unwrap();
        let map = self.binding_control_store().load()?;
        let source = map
            .key_for_worker(&context.project_scope, &context.app_scope_id, worker)?
            .ok_or("IDENTITY_MAP_NOT_FOUND")?;
        let original = map
            .slot(source)
            .and_then(|slot| slot.active_binding())
            .expect("active key");
        let mut value = original.clone();
        let target = intent
            .as_ref()
            .map(|intent| intent.requested_name.clone())
            .unwrap_or_else(|| source.name.clone());
        let _display_slot = name_resolution::display_slot(&target)?;
        if intent
            .as_ref()
            .is_some_and(|intent| !intent.user_authorized)
        {
            return Err("NAME_AUTHORIZATION_REQUIRED".into());
        }
        let mut cross_scope_sources = if intent
            .as_ref()
            .is_some_and(|intent| intent.user_authorized)
        {
            let mut sources: Vec<_> = map
                .project_currents_for_name(&context.project_scope, &target)
                .into_iter()
                .filter(|slot| slot.identity_key.app_scope_id != context.app_scope_id)
                .filter_map(BindingControlSet::cross_scope_source_ref)
                .collect();
            if let Some(facts) = facts {
                let cwd = pane
                    .as_ref()
                    .map(|pane| pane.cwd.as_str())
                    .or(Some(value.invocation_cwd.as_str()));
                if let Some(source_slot) = map.project_current_for_facts_excluding_app(
                    &context.project_scope,
                    Some(&context.app_scope_id),
                    facts,
                    cwd,
                )? {
                    if source_slot.identity_key.app_scope_id != context.app_scope_id {
                        if let Some(source) = BindingControlSet::cross_scope_source_ref(source_slot)
                        {
                            sources.push(source);
                        }
                    }
                }
            }
            let mut seen = std::collections::BTreeSet::new();
            sources.retain(|source| {
                seen.insert((
                    source.key.app_scope_id.as_str().to_owned(),
                    source.key.name.clone(),
                ))
            });
            sources
        } else {
            Vec::new()
        };
        if let Some(facts) = facts {
            value.host.agent_kind = facts.agent_kind.clone().or(value.host.agent_kind);
            value.host.profile = facts.profile.clone().or(value.host.profile);
        }
        if let Some(pane) = &pane {
            value.invocation_cwd = pane.cwd.clone();
        }
        Self::prepare_control_value(&map, source, &mut value)?;
        let session = value
            .binding
            .session_id
            .as_ref()
            .ok_or("IDENTITY_SESSION_MISSING")?
            .to_string();
        let state = runtime.state.lock().unwrap();
        let route = value.binding.route_scope();
        let role = if intent.is_some() && target == "Jenny" {
            Some(RoleEffect {
                action: RoleAction::SetMaster,
                authorization: authorization.clone().ok_or("ROLE_AUTHORIZATION_MISSING")?,
            })
        } else if intent.is_some()
            && current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker)
        {
            Some(RoleEffect {
                action: RoleAction::ClearMaster,
                authorization: authorization.ok_or("ROLE_AUTHORIZATION_MISSING")?,
            })
        } else {
            None
        };
        let settled = match role.as_ref().map(|role| &role.action) {
            Some(RoleAction::SetMaster) => {
                current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker)
            }
            Some(RoleAction::ClearMaster) => current_master_grant(&state, Some(&route)).is_none(),
            None => true,
        };
        if matches!(
            role.as_ref().map(|role| &role.action),
            Some(RoleAction::SetMaster)
        ) && !settled
        {
            super::tm_layout::require_master_position(&value)?;
        }
        if source.name == target
            && original == &value
            && settled
            && state
                .name_occupancy(context.project_scope.as_str(), &target)
                .is_some_and(|holder| holder.worker_id == worker && holder.session_id == session)
        {
            let is_current_master = current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker);
            drop(state);
            let physical = rename_prepared_anchor(&value, &value.invocation_cwd, &target, is_current_master)?;
            return Ok(
                json!({"outcome":"committed","name":target,"session_id":session,
                    "physical_anchor":physical.readback(value.physical_name.clone()),
                    "effects":{"grant_committed":role.is_some()}}),
            );
        }
        let key = IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            &target,
        );
        let previous = map.slot(&key).and_then(|slot| slot.active_binding());
        if intent.as_ref().is_some_and(|intent| intent.user_authorized) {
            if let Some(previous) = previous {
                value.replace_rex_holder(previous)?;
            }
            for source in &cross_scope_sources {
                if let Some(current) = map.slot(&source.key).and_then(|slot| slot.active_binding()) {
                    value.preserve_rex_fence(current);
                    if source.key.name.eq_ignore_ascii_case(&target) {
                        value.replace_rex_holder(current)?;
                    }
                }
            }
        }
        let mut local_source_names: Vec<_> = map
            .slots()
            .filter(|slot| {
                slot.identity_key.project_scope == context.project_scope
                    && slot.identity_key.app_scope_id == context.app_scope_id
                    && slot.identity_key.name != target
                    && slot.active_binding().is_some_and(|current| {
                        current.binding.agent_id.as_str() == worker
                    })
            })
            .map(|slot| slot.identity_key.name.clone())
            .collect();
        local_source_names.sort();
        local_source_names.dedup();
        cross_scope_sources.sort_by(|left, right| {
            (
                left.key.app_scope_id.as_str(),
                left.key.name.as_str(),
            )
                .cmp(&(
                    right.key.app_scope_id.as_str(),
                    right.key.name.as_str(),
                ))
        });
        let effect = PendingEffect {
            kind: if cross_scope_sources.is_empty() {
                PendingEffectKind::Name
            } else {
                PendingEffectKind::NameAcrossScopes {
                    sources: cross_scope_sources,
                }
            },
            role: role.clone(),
            effect_id: format!("name:{}:{operation}", context.project_scope.as_str()),
            operation_id: operation.into(),
            project_scope: context.project_scope.clone(),
            app_scope_id: context.app_scope_id.clone(),
            name: target.clone(),
            previous_worker_id: previous.map(|value| value.binding.agent_id.to_string()),
            previous_session_id: previous
                .and_then(|value| value.binding.session_id.as_ref())
                .map(ToString::to_string),
            target_worker_id: worker.into(),
            target_session_id: session.clone(),
            occupancy_revision: state
                .name_occupancy_revision(context.project_scope.as_str(), &target)
                .checked_add(1)
                .ok_or("NAME_REVISION_EXHAUSTED")?,
            source_names: local_source_names,
            moves_existing_names: intent
                .as_ref()
                .is_some_and(|intent| intent.moves_existing_names),
            pane_anchor: pane,
            created_ms: now_ms(),
        };
        let is_current_master = current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker);
        drop(state);
        let rename = rename_prepared_anchor(&value, &value.invocation_cwd, &target, is_current_master)?;
        let physical = rename.readback(value.physical_name.clone());
        let committed = match self.commit_control_change_locked(
            super::binding_store::ControlChange::MoveNameWithEffect {
                target: key,
                value,
                source_names: effect.source_names.clone(),
                expected_generation: None,
                effect: effect.clone(),
                companion: None,
            },
        ) {
            Ok(committed) => committed,
            Err(error) => return Err(match rename.rollback() {
                Ok(_) => error,
                Err(rollback) => format!("{error}; TMUX_SESSION_RENAME_ROLLBACK_FAILED: {rollback}"),
            }),
        };
        if committed.durability == super::binding_store::Durability::Unknown {
            return Err(
                "BINDING_CONTROL_DURABILITY_UNKNOWN: map published; effects remain pending".into(),
            );
        }
        let published = self.binding_control_store().load()?;
        self.recover_pending_effect_locked(&published, &effect)
            .map_err(|error| format!("CONTROL_COMMITTED_EFFECTS_PENDING: {error}"))?;
        Ok(
            json!({"outcome":"committed","name":target,"session_id":session,
                "physical_anchor":physical,"effects":{"grant_committed":role.is_some()}}),
        )
    }
}
