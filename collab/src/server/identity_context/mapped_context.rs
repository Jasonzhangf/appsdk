use crate::server::binding_slot::{
    IdentityBindingKey, PendingEffect, PendingEffectKind, RoleAction, RoleAuthorization, RoleEffect,
};

include!("anchor_rename.rs");
include!("communication.rs");

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
            return self.current_mapped_context(&context, &request);
        }
        if let Err(error) = self.reconcile_current_dsh_session(&context, &facts) {
            return (self.host.clone(), Resp::err(format!("{error:#}")));
        }
        let _identity = self.identity_gate.lock().unwrap();
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
                Some("restore_master" | "restore_peer" | "restore_name")
            ) {
                return (
                    self.host.clone(),
                    Resp::err(
                        "IDENTITY_APPROVAL_ACTION_UNSUPPORTED: use current name or role intent",
                    ),
                );
            }
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
        }
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
            .with_role_intent(&request);
        let reconciled = self.reconcile_identity_context(
            context.clone(),
            facts.clone(),
            Some(outer),
            true,
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
        let raw_authorization = request
            .grant_approval
            .as_ref()
            .or(request.approval.as_ref());
        let mut intent = match name_resolution::parse_name_intent(raw_authorization) {
            Ok(intent) => intent,
            Err(error) => return (runtime, Resp::err(error)),
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
        if let Err(error) = name_result {
            return (runtime, Resp::err(error));
        }
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
        if request.operation_id.is_empty() && communication.is_none() {
            return (runtime, response);
        }
        let outcome = communication.as_ref().map(communication_operation_outcome).unwrap_or("completed");
        let mut result =
            json!({"result":{
                "operation_id":request.operation_id,"action":"context","invocation":request.invocation,
                "outcome":outcome,"phase":"context_complete","committed_phases":[],"failed_phase":null,
                "requires":{"kind":null,"fields":[],"sources":{},"approval":null,"repair_invocation":null},
                "snapshot":response.data["snapshot"],"owner_readback":{}
            }});
        if let Some(communication) = communication { result["result"]["owner_readback"]["communication"] = communication; }
        (
            runtime,
            Resp { ok: outcome == "completed", error: None, data: result },
        )
    }

    fn current_mapped_context(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> (Arc<Server>, Resp) {
        let identity = match self.current_session_identity_at_cwd(
            context,
            &request.facts,
            request.invocation_cwd.as_deref(),
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
        let outcome = communication.as_ref().map(communication_operation_outcome).unwrap_or("completed");
        let mut result = json!({"result":{"operation_id":request.operation_id,"action":"context","invocation":"query",
            "outcome":outcome,"phase":null,"committed_phases":[],"failed_phase":null,
            "requires":{"kind":null,"fields":[],"sources":{},"approval":null,"repair_invocation":null},"snapshot":snapshot.data,"owner_readback":{}}});
        if let Some(communication) = communication { result["result"]["owner_readback"]["communication"] = communication; }
        (
            runtime,
            Resp { ok: outcome == "completed", error: None, data: result },
        )
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
            } => (
                worker_id.as_str(),
                token.as_str(),
                None,
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
            let Some(grant) = current_master_grant(&state, Some(&route))
                .filter(|grant| grant.agent_id.as_str() == worker)
            else {
                return Resp::err(
                    "MASTER_DELEGATE_DENIED: caller must hold the current master grant",
                );
            };
            (
                target.to_owned(),
                "Jenny".to_owned(),
                RoleAuthorization::Delegate {
                    actor: worker.into(),
                    target: target.into(),
                    grant_id: grant.resource_id().into(),
                    grant_generation: grant.resource_generation(),
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
                let brief = match role_brief(runtime, &state, &control, worker) {
                    Ok(brief) => brief, Err(error) => return Resp::err(error),
                };
                let mut response = json!({"master":current_master_worker_id(&state, Some(&route)),"scope":scope_view(Some(&route)),
                    "name_resolution":outcome,"mode":"user_approved_self_promotion","role_brief":brief});
                if matches!(request, Req::MasterClear { .. }) {
                    response["cleared"] = json!(true);
                    response["was_empty"] = json!(previous.is_none());
                    response["previous_worker_id"] = json!(previous);
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
        name_resolution::display_slot(&target)?;
        if intent
            .as_ref()
            .is_some_and(|intent| !intent.user_authorized)
        {
            return Err("NAME_AUTHORIZATION_REQUIRED".into());
        }
        if let Some(facts) = facts {
            value.host.agent_kind = facts.agent_kind.clone().or(value.host.agent_kind);
            value.host.profile = facts.profile.clone().or(value.host.profile);
        }
        if let Some(pane) = &pane {
            value.invocation_cwd = pane.cwd.clone();
        }
        if let Some(endpoint) = &value.binding.tmux_endpoint {
            value.physical_name = Some(crate::client::adapters::tmux::observe_session_name(
                endpoint,
            )?);
        }
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
            return Ok(
                json!({"outcome":"committed","name":target,"session_id":session,"effects":{"grant_committed":role.is_some()}}),
            );
        }
        let key = IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            &target,
        );
        let previous = map.slot(&key).and_then(|slot| slot.active_binding());
        let effect = PendingEffect {
            kind: PendingEffectKind::Name,
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
            source_names: vec![source.name.clone()],
            moves_existing_names: intent
                .as_ref()
                .is_some_and(|intent| intent.moves_existing_names),
            pane_anchor: pane,
            created_ms: now_ms(),
        };
        let companion = {
            previous
                .filter(|previous| previous.binding.agent_id.as_str() != worker)
                .map(|previous| {
                    let mut candidate = map.clone();
                    if effect.moves_existing_names {
                        candidate.move_name(&key, value.clone(), &effect.source_names, None)
                            .map_err(|error| error.to_string())?;
                    }
                    let ordinary = candidate.ordinary_name(
                        &context.project_scope,
                        &context.app_scope_id,
                        previous.binding.agent_id.as_str(),
                    )?;
                    let key = IdentityBindingKey::new(
                        context.project_scope.clone(),
                        context.app_scope_id.clone(),
                        &ordinary,
                    );
                    let mut outgoing = previous.clone();
                    if let Some(endpoint) = &outgoing.binding.tmux_endpoint {
                        outgoing.physical_name = crate::client::adapters::tmux::session_name_if_live(endpoint)?
                            .map(|_| super::tm_layout::physical_name(&outgoing, &outgoing.invocation_cwd, &ordinary))
                            .transpose()?.flatten();
                    }
                    let outgoing_effect = PendingEffect {
                        role: None,
                        effect_id: format!("{}:outgoing", effect.effect_id),
                        name: ordinary.clone(),
                        previous_worker_id: None,
                        previous_session_id: None,
                        target_worker_id: outgoing.binding.agent_id.to_string(),
                        target_session_id: outgoing
                            .binding
                            .session_id
                            .as_ref()
                            .ok_or("IDENTITY_SESSION_MISSING")?
                            .to_string(),
                        occupancy_revision: state
                            .name_occupancy_revision(context.project_scope.as_str(), &ordinary)
                            .checked_add(1)
                            .ok_or("NAME_REVISION_EXHAUSTED")?,
                        source_names: Vec::new(),
                        moves_existing_names: false,
                        pane_anchor: None,
                        ..effect.clone()
                    };
                    Ok::<_, String>((key, outgoing, outgoing_effect))
                })
                .transpose()?
        };
        let is_current_master = current_master_worker_id(&state, Some(&route)).as_deref() == Some(worker);
        drop(state);
        let rename = rename_prepared_anchor(&value, &value.invocation_cwd, &target, is_current_master,
            companion.as_ref().map(|(key, value, _)| (value, key.name.as_str())))?;
        let committed = match self.commit_control_change_locked(
            super::binding_store::ControlChange::MoveNameWithEffect {
                target: key,
                value,
                source_names: effect.source_names.clone(),
                expected_generation: None,
                effect: effect.clone(),
                companion: companion.clone(),
            },
        ) {
            Ok(committed) => committed,
            Err(error) => return Err(match rename.map(|rename| rename.rollback()).transpose() {
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
        if let Some((_, _, outgoing)) = companion {
            self.recover_pending_effect_locked(&published, &outgoing)
                .map_err(|error| format!("CONTROL_COMMITTED_EFFECTS_PENDING: {error}"))?;
        }
        Ok(
            json!({"outcome":"committed","name":target,"session_id":session,"effects":{"grant_committed":role.is_some()}}),
        )
    }
}
