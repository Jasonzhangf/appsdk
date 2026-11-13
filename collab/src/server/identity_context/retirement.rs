// Explicit human retirement (`collab context --approve-grant
// '{"action":"retire_name","requested_name":"Bob","authorization":"..."}'`).
//
// This is the approved E1/E2/E3 path. A normal peer may carry the explicit human
// instruction; no master approval or second confirmation is required. Ordinary
// names and Jenny can be released. The exact target binding is retired through the
// existing `UnregisterWithEffect` / bounded pending effect, so tasks,
// messages, resources and the host are preserved. A completed command receipt
// makes a retry of the same operation return its recorded result instead of
// retiring a successor that later took the name.

use super::*;

enum HumanRetireFailure {
    /// Nothing was published: return the typed refusal unchanged.
    Refused(String),
    /// The map published the release but a later step failed. The effect stays
    /// pending and a retry continues from the real phase.
    Committed(String),
}

impl ProjectRuntimeManager {
    /// Dedicated dispatch for the `retire_name` approval. The caller is not
    /// registered or renamed here: a retirement must never mask itself by
    /// auto-registering the retired caller.
    pub(super) fn human_retire_name(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
        approval: &serde_json::Value,
    ) -> (Arc<Server>, Resp) {
        let Some(requested_name) = approval
            .get("requested_name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
        else {
            return (self.host.clone(), Resp::err("IDENTITY_RETIRE_NAME_REQUIRED"));
        };
        if requested_name != "Jenny" && name_resolution::display_slot(&requested_name).is_err() {
            return (
                self.host.clone(),
                Resp::err("IDENTITY_RETIRE_NAME_NOT_ORDINARY: retire_name releases ordinary names or Jenny"),
            );
        }
        let Some(runtime) = self
            .routes
            .lock()
            .unwrap()
            .get(&(
                context.app_scope_id.to_string(),
                context.project_scope.as_str().to_owned(),
            ))
            .and_then(|route| route.runtime.clone())
        else {
            return (
                self.host.clone(),
                Resp::err(
                    "IDENTITY_CONTEXT_READBACK_UNAVAILABLE: the retiring project is not loaded",
                ),
            );
        };
        let operation_id = if request.operation_id.trim().is_empty() {
            format!(
                "retire-name-{}-{}-{}",
                std::process::id(),
                now_ms(),
                rand::random::<u64>()
            )
        } else {
            request.operation_id.clone()
        };
        // Replay guard: a recorded receipt for this operation returns the
        // original result. It never selects a successor by name.
        if let Some(receipt) = self.retire_receipt_outcome(&runtime, &operation_id) {
            if receipt["result"]["name"].as_str() != Some(requested_name.as_str()) {
                return (
                    runtime,
                    Resp::err(format!(
                        "IDENTITY_OPERATION_CONFLICT: operation {operation_id} already names {}",
                        receipt["result"]["name"]
                            .as_str()
                            .unwrap_or("another target")
                    )),
                );
            }
            if let Err(error) =
                self.resume_pending_human_retire(context, &operation_id, &requested_name)
            {
                return (
                    runtime,
                    Resp::err_data(
                        format!("CONTROL_COMMITTED_EFFECTS_PENDING: {error}"),
                        json!({"control_committed": true}),
                    ),
                );
            }
            let view = match self.current_retire_view(
                context,
                &requested_name,
                Some(&operation_id),
                true,
            ) {
                Ok(view) => view,
                Err(error) => return (runtime, Resp::err(error)),
            };
            return (
                runtime,
                human_retire_result(
                    request,
                    &operation_id,
                    "completed",
                    vec!["retired".into()],
                    view,
                ),
            );
        }
        match self.publish_and_consume_human_retire(context, &requested_name, &operation_id, &runtime)
        {
            Ok(()) => {
                let view = match self.current_retire_view(
                    context,
                    &requested_name,
                    Some(&operation_id),
                    true,
                ) {
                    Ok(view) => view,
                    Err(error) => return (runtime, Resp::err(error)),
                };
                (
                    runtime,
                    human_retire_result(
                        request,
                        &operation_id,
                        "completed",
                        vec!["retired".into()],
                        view,
                    ),
                )
            }
            Err(HumanRetireFailure::Refused(message)) => {
                (runtime, Resp::err(message))
            }
            Err(HumanRetireFailure::Committed(message)) => {
                let data = self
                    .current_retire_view(context, &requested_name, Some(&operation_id), false)
                    .map(|snapshot| json!({"control_committed": true, "snapshot": snapshot}))
                    .unwrap_or_else(|_| json!({"control_committed": true}));
                (runtime, Resp::err_data(message, data))
            }
        }
    }

    fn publish_and_consume_human_retire(
        &self,
        context: &ProjectContext,
        requested_name: &str,
        operation_id: &str,
        runtime: &Arc<Server>,
    ) -> Result<(), HumanRetireFailure> {
        let _register = self.register_gate.lock().unwrap();
        let map = self
            .binding_control_store()
            .load()
            .map_err(HumanRetireFailure::Refused)?;
        // Resume an already published effect for this operation instead of
        // publishing a second release.
        if let Some(existing) = map
            .pending_effects()
            .find(|effect| {
                matches!(
                    effect.kind,
                    crate::server::binding_slot::PendingEffectKind::HumanRetire { .. }
                )
                    && effect.operation_id == operation_id
                    && effect.project_scope == context.project_scope
                    && effect.app_scope_id == context.app_scope_id
                    && effect.name == requested_name
            })
            .cloned()
        {
            return self
                .recover_pending_effect_locked(&map, &existing)
                .map_err(HumanRetireFailure::Committed);
        }
        // An operation ID belongs to one intent. Check existing command
        // receipts before publishing this release so a reused ID cannot
        // release the target and fail only at the State commit. An exact
        // retirement receipt is a completed replay, not a conflict.
        if let Some(receipt) = runtime
            .state
            .lock()
            .unwrap()
            .global
            .command_receipts
            .values()
            .find(|receipt| receipt.operation_id.as_str() == operation_id)
        {
            let exact = receipt.command_id.as_str() == format!("retire_name:{operation_id}")
                && receipt.outcome["result"]["name"].as_str() == Some(requested_name)
                && receipt.outcome["result"]["project_scope"].as_str()
                    == Some(context.project_scope.as_str())
                && receipt.outcome["result"]["app_scope_id"].as_str()
                    == Some(context.app_scope_id.as_str());
            if exact {
                return Ok(());
            }
            let actual = receipt.outcome["result"]["name"]
                .as_str()
                .unwrap_or("another target");
            return Err(HumanRetireFailure::Refused(format!(
                "IDENTITY_OPERATION_CONFLICT: operation {operation_id} already names {actual}"
            )));
        }
        if let Some(conflict) = map
            .pending_effects()
            .find(|effect| effect.operation_id == operation_id)
        {
            return Err(HumanRetireFailure::Refused(format!(
                "IDENTITY_OPERATION_CONFLICT: operation {operation_id} already names {}",
                conflict.name
            )));
        }
        let key = crate::server::binding_slot::IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            requested_name,
        );
        let Some((value, generation)) = map.slot(&key).and_then(|slot| {
            slot.active_binding()
                .map(|value| (value.clone(), slot.generation()))
        })
        else {
            return Err(HumanRetireFailure::Refused(format!(
                "IDENTITY_RETIRE_NAME_NOT_HELD: {requested_name} has no current holder"
            )));
        };
        let Some(session) = value.binding.session_id.as_ref() else {
            return Err(HumanRetireFailure::Refused("IDENTITY_SESSION_MISSING".into()));
        };
        let effect = crate::server::binding_slot::PendingEffect {
            kind: crate::server::binding_slot::PendingEffectKind::HumanRetire {
                binding_id: value.binding.binding_id.clone(),
                generation,
            },
            role: None,
            effect_id: format!(
                "human-retire:{}:{}",
                context.project_scope.as_str(),
                operation_id
            ),
            operation_id: operation_id.to_owned(),
            project_scope: key.project_scope.clone(),
            app_scope_id: key.app_scope_id.clone(),
            name: key.name.clone(),
            previous_worker_id: Some(value.binding.agent_id.to_string()),
            previous_session_id: Some(session.to_string()),
            target_worker_id: value.binding.agent_id.to_string(),
            target_session_id: session.to_string(),
            occupancy_revision: generation.max(1),
            source_names: Vec::new(),
            moves_existing_names: false,
            pane_anchor: None,
            created_ms: now_ms(),
        };
        let committed = self
            .commit_control_change_locked(crate::server::binding_store::ControlChange::UnregisterWithEffect {
                key,
                effect: effect.clone(),
            })
            .map_err(HumanRetireFailure::Refused)?;
        if committed.durability == crate::server::binding_store::Durability::Unknown {
            return Err(HumanRetireFailure::Committed(
                "BINDING_CONTROL_DURABILITY_UNKNOWN: retirement published; effects remain pending"
                    .into(),
            ));
        }
        let published = self
            .binding_control_store()
            .load()
            .map_err(HumanRetireFailure::Committed)?;
        self.recover_pending_effect_locked(&published, &effect)
            .map_err(|error| {
                HumanRetireFailure::Committed(format!(
                    "CONTROL_COMMITTED_EFFECTS_PENDING: {error}"
                ))
            })?;
        Ok(())
    }

    fn resume_pending_human_retire(
        &self,
        context: &ProjectContext,
        operation_id: &str,
        requested_name: &str,
    ) -> Result<(), String> {
        let _register = self.register_gate.lock().unwrap();
        let map = self.binding_control_store().load()?;
        let Some(existing) = map
            .pending_effects()
            .find(|effect| {
                matches!(
                    effect.kind,
                    crate::server::binding_slot::PendingEffectKind::HumanRetire { .. }
                )
                    && effect.operation_id == operation_id
                    && effect.project_scope == context.project_scope
                    && effect.app_scope_id == context.app_scope_id
                    && effect.name == requested_name
            })
            .cloned()
        else {
            return Ok(());
        };
        self.recover_pending_effect_locked(&map, &existing)
    }

    fn retire_receipt_outcome(
        &self,
        runtime: &Arc<Server>,
        operation_id: &str,
    ) -> Option<serde_json::Value> {
        let command_id = format!("retire_name:{operation_id}");
        runtime
            .state
            .lock()
            .unwrap()
            .global
            .command_receipts
            .values()
            .find(|receipt| {
                receipt.operation_id.as_str() == operation_id
                    && receipt.command_id.as_str() == command_id
            })
            .map(|receipt| receipt.outcome.clone())
    }

    fn query_human_retirement(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> Option<(Arc<Server>, Resp)> {
        let operation_id = request.operation_id.trim();
        if operation_id.is_empty() {
            return None;
        }
        let approval = request
            .grant_approval
            .as_ref()
            .or(request.approval.as_ref());
        let explicit = approval
            .and_then(|approval| approval.get("action"))
            .and_then(serde_json::Value::as_str)
            == Some("retire_name");
        let runtime = self
            .routes
            .lock()
            .unwrap()
            .get(&(
                context.app_scope_id.to_string(),
                context.project_scope.as_str().to_owned(),
            ))
            .and_then(|route| route.runtime.clone())?;
        let receipt = self.retire_receipt_outcome(&runtime, operation_id);
        let map = match self.binding_control_store().load() {
            Ok(map) => map,
            Err(error) => return Some((runtime, Resp::err(error))),
        };
        let pending = map
            .pending_effects()
            .find(|effect| {
                matches!(
                    effect.kind,
                    crate::server::binding_slot::PendingEffectKind::HumanRetire { .. }
                )
                    && effect.operation_id == operation_id
                    && effect.project_scope == context.project_scope
                    && effect.app_scope_id == context.app_scope_id
            })
            .map(|effect| effect.name.clone());
        if !explicit && receipt.is_none() && pending.is_none() {
            return None;
        }
        let requested_name = approval
            .and_then(|approval| approval.get("requested_name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                receipt
                    .as_ref()
                    .and_then(|outcome| outcome["result"]["name"].as_str())
                    .map(str::to_owned)
            })
            .or_else(|| pending.clone());
        let Some(requested_name) = requested_name else {
            return Some((
                runtime,
                Resp::err("IDENTITY_RETIRE_NAME_REQUIRED: query needs an operation or requested_name"),
            ));
        };
        let snapshot =
            match self.current_retire_view(context, &requested_name, Some(operation_id), receipt.is_some()) {
                Ok(snapshot) => snapshot,
                Err(error) => return Some((runtime, Resp::err(error))),
            };
        let (outcome, phase) = match (receipt.is_some(), pending.is_some()) {
            (true, true) => ("effects_pending", Some("effects")),
            (true, false) => ("completed", Some("retired")),
            (false, true) => ("pending", Some("retired")),
            (false, false) => ("not_found", None),
        };
        Some((
            runtime,
            typed_operation_resp(crate::proto::ContextOperationResult {
                operation_id: operation_id.to_owned(),
                invocation: request.invocation.clone(),
                action: "retire_name".into(),
                phase: phase.map(str::to_owned),
                outcome: outcome.into(),
                committed_phases: receipt
                    .as_ref()
                    .map(|_| vec!["retired".into()])
                    .unwrap_or_default(),
                failed_phase: pending
                    .as_ref()
                    .map(|_| "effects".to_owned()),
                requires: crate::proto::ContextOperationRequires {
                    kind: pending
                        .as_ref()
                        .map(|_| "effects".to_owned()),
                    fields: Vec::new(),
                    sources: Default::default(),
                    approval: None,
                    repair_invocation: Some(format!(
                        "collab context --query --op {operation_id} --approve-grant \
                         '{{\"action\":\"retire_name\",\"requested_name\":\"{requested_name}\"}}'"
                    )),
                },
                snapshot: Some(snapshot),
                owner_readback: Default::default(),
            }),
        ))
    }

    fn current_retire_view(
        &self,
        context: &ProjectContext,
        name: &str,
        operation_id: Option<&str>,
        receipt: bool,
    ) -> Result<serde_json::Value, String> {
        let map = self.binding_control_store().load()?;
        let current = map
            .current_for_name(&context.project_scope, &context.app_scope_id, name)?
            .and_then(|slot| slot.active_binding())
            .map(|value| {
                json!({
                    "worker_id": value.binding.agent_id.as_str(),
                    "session_id": value.binding.session_id.as_ref().map(|id| id.as_str()),
                })
            });
        let pending = map
            .pending_effects()
            .any(|effect| {
                matches!(effect.kind, crate::server::binding_slot::PendingEffectKind::HumanRetire { .. })
                    && operation_id.is_some_and(|id| effect.operation_id == id)
                    && effect.name == name
            });
        Ok(json!({
            "identity": {
                "name": name,
                "status": if current.is_none() { "released" } else { "held" },
                "current": current,
            },
            "operations": {
                "operation_id": operation_id,
                "receipt": receipt,
                "pending": pending,
            },
        }))
    }
}

fn human_retire_result(
    request: &IdentityContextRequest,
    operation_id: &str,
    outcome: &str,
    committed_phases: Vec<String>,
    snapshot: serde_json::Value,
) -> Resp {
    typed_operation_resp(crate::proto::ContextOperationResult {
        operation_id: operation_id.to_owned(),
        invocation: request.invocation.clone(),
        action: "retire_name".into(),
        phase: Some("retired".into()),
        outcome: outcome.into(),
        committed_phases,
        failed_phase: None,
        requires: crate::proto::ContextOperationRequires {
            kind: None,
            fields: Vec::new(),
            sources: Default::default(),
            approval: None,
            repair_invocation: None,
        },
        snapshot: Some(snapshot),
        owner_readback: Default::default(),
    })
}
