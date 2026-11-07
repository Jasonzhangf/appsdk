use crate::server::notification_contract::SelectedDeliveryBinding;

fn matching_communication_receive<'a>(
    state: &'a State, binding: &SelectedDeliveryBinding, message_id: &str,
) -> Option<&'a state::ReceiveReceipt> {
    state.receive_receipts.values().find(|receipt|
        receipt.worker_id == binding.binding.agent_id.as_str()
            && receipt.route_scope.as_ref() == Some(&binding.binding.route_scope())
            && receipt.message_ids.iter().any(|id| id == message_id))
}

fn current_delivery_outcome(state: &State, binding: &SelectedDeliveryBinding, message: &Message) -> &'static str {
    if state.notification_delivery_bindings.get(&message.id) != Some(binding) { return "unknown"; }
    let accepted = state.notification_delivery_accepted.get(&message.id).copied();
    let failure = state.notification_delivery_failures.get(&message.id);
    if accepted.is_some_and(|accepted| accepted >= message.last_wake_attempt_ms
        && failure.is_none_or(|failure| failure.failed_ms < accepted)) { return "accepted"; }
    if failure.is_some_and(|failure| failure.failed_ms >= message.last_wake_attempt_ms && failure.retryable) {
        return "rejected";
    }
    "unknown"
}

fn project_communication_exchange(state: &State, binding: &SelectedDeliveryBinding, message: &Message) -> serde_json::Value {
    let principal = binding.binding.agent_id.as_str();
    let exact = state.notification_delivery_bindings.get(&message.id) == Some(binding);
    let delivery = current_delivery_outcome(state, binding, message);
    let receive = matching_communication_receive(state, binding, &message.id);
    let reply = state.msgs.values().filter(|reply|
        reply.from == principal && reply.to == principal && reply.mtype == "reply"
            && reply.in_reply_to.as_deref() == Some(message.id.as_str()))
        .min_by_key(|reply| (reply.created_ms, &reply.id));
    let recovered = exact && delivery == "accepted" && receive.is_some() && reply.is_some();
    json!({
        "state":if recovered { "recovered" } else if !exact { "unknown" }
            else if delivery == "rejected" { "failed" } else if delivery == "accepted" { "pending" } else { "unknown" },
        "classification":if recovered { "healthy" } else if delivery == "rejected" { "broken" } else { "unknown" },
        "message_id":message.id,"delivery":delivery,
        "receive_id":receive.map(|receipt| &receipt.receive_id),
        "reply_message_id":reply.map(|reply| &reply.id),
        "error":if delivery == "accepted" { None } else {
            state.notification_delivery_failures.get(&message.id).map(|failure| &failure.error)
        }
    })
}

fn communication_projection(classification: &str, state: &str) -> serde_json::Value {
    json!({"state":state,"classification":classification,"message_id":null,
        "delivery":"not_attempted","receive_id":null,"reply_message_id":null,"error":null})
}

fn communication_operation_outcome(communication: &serde_json::Value) -> &'static str {
    match communication["state"].as_str() {
        Some("not_needed" | "recovered") => "completed",
        Some("pending") => "pending",
        Some("failed") => "explicit_failure",
        _ => "unknown",
    }
}

impl ProjectRuntimeManager {
    fn mapped_communication(
        &self, runtime: &Arc<Server>, context: &ProjectContext,
        worker_id: &str, token: &str, manual: bool, query: bool,
    ) -> Result<Option<serde_json::Value>, String> {
        if !manual && !query { return Ok(None); }
        let control = self.binding_control_store().load()?;
        let binding = control.current_for_worker(&context.project_scope, &context.app_scope_id, worker_id)?
            .and_then(SelectedDeliveryBinding::from_slot)
            .ok_or_else(|| "IDENTITY_MAP_NOT_FOUND: communication caller has no current binding".to_owned())?;
        let (exchange, latest, worker) = {
            let state = runtime.state.lock().unwrap();
            let exchange = state.msgs.values().filter(|message|
                message.from == worker_id && message.to == worker_id
                    && message.subject.as_deref() == Some("communication-recovery"))
                .max_by_key(|message| (message.created_ms, &message.id))
                .map(|message| (project_communication_exchange(&state, &binding, message),
                    state.notification_delivery_bindings.get(&message.id) == Some(&binding)));
            let latest = state.msgs.values().filter(|message| message.to == worker_id && message.wake_attempt_count > 0)
                .max_by_key(|message| (message.last_wake_attempt_ms, message.created_ms, &message.id))
                .map(|message| (current_delivery_outcome(&state, &binding, message),
                    matching_communication_receive(&state, &binding, &message.id).is_some()));
            (exchange, latest, state.workers.get(worker_id).cloned())
        };
        if query { return Ok(exchange.map(|(exchange, _)| exchange)); }
        let endpoint_missing = worker.as_ref().is_some_and(|worker|
            worker_identity_and_endpoint(runtime, worker).1 == Some(false));
        let classification = if endpoint_missing || latest.is_some_and(|(outcome, _)| outcome == "rejected") {
            "broken"
        } else if latest.is_some_and(|(outcome, _)| outcome == "unknown") { "unknown" }
        else if latest == Some(("accepted", true))
            || exchange.as_ref().is_some_and(|(exchange, exact)| *exact && exchange["state"] == "recovered") { "healthy" }
        else { "unknown" };
        if let Some((exchange, exact)) = exchange {
            let exact_pending = exchange["state"] == "pending";
            let uncertain = exchange["state"] == "unknown" && exchange["delivery"] != "not_attempted";
            if exact && (exact_pending || uncertain || (exchange["state"] == "recovered" && classification == "healthy")) {
                return Ok(Some(exchange));
            }
        }
        if classification == "healthy" { return Ok(Some(communication_projection("healthy", "not_needed"))); }
        if classification == "unknown" { return Ok(Some(communication_projection("unknown", "unknown"))); }

        // Use the ordinary authenticated Send owner. No identity/state lock is
        // held while the selected adapter or the independent receiver runs.
        let nonce = format!("communication-{}-{}", std::process::id(), rand::random::<u128>());
        let command = crate::proto::CommandEnvelope::new(
            crate::identity::CommandId::new(nonce.clone()).map_err(|error| error.to_string())?,
            crate::identity::OperationId::new(nonce).map_err(|error| error.to_string())?,
            binding.binding.binding_id.clone(), binding.binding.route_scope(), None, None, None, None);
        let response = dispatch_with_route_context(runtime, Req::Send {
            from:worker_id.into(),worker_id:Some(worker_id.into()),token:Some(token.into()),command:Some(command),
            to:worker_id.into(),mtype:"notify".into(),subject:Some("communication-recovery".into()),
            body:"Read this message with collab recv using receive-id recover-<message-id>. Reply once with ordinary sendmessage, type reply, in-reply-to this message, subject communication-recovery-reply, body communication recovery received.".into(),
            in_reply_to:None,delivery:"immediate".into(),reply_policy:None,
            target_session:binding.binding.session_id.as_ref().map(ToString::to_string),
        }, Some(context.clone()));
        let Some(id) = response.data["msg_id"].as_str() else {
            let mut result = communication_projection("broken", "failed");
            result["error"] = json!(response.error.or_else(|| response.data["notification_error"].as_str().map(str::to_owned)));
            return Ok(Some(result));
        };
        let state = runtime.state.lock().unwrap();
        let message = state.msgs.get(id).ok_or_else(|| "COMMUNICATION_MESSAGE_NOT_FOUND: ordinary Send returned no current message".to_owned())?;
        let mut result = project_communication_exchange(&state, &binding, message);
        if result["state"] != "recovered" { result["classification"] = json!("broken"); }
        Ok(Some(result))
    }
}
