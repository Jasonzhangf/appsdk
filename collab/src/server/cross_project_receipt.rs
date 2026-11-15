//! Local cross-project references route reads, never duplicate mailbox state.
use super::*;
use crate::proto::{MessageRef, MessageReference};

/// Original routing addresses, committed only in the recipient-owned mailbox.
/// They identify names; the identity map remains the sole holder authority.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Addresses {
    pub sender: binding_slot::IdentityBindingKey,
    pub recipient: binding_slot::IdentityBindingKey,
}

fn holder<'a>(
    control: &'a binding_slot::BindingControlSet,
    address: &binding_slot::IdentityBindingKey,
) -> Result<Option<&'a str>, Resp> {
    Ok(control
        .current_for_name(&address.project_scope, &address.app_scope_id, &address.name)
        .map_err(Resp::err)?
        .and_then(|slot| slot.active_binding())
        .map(|current| current.binding.agent_id.as_str()))
}

pub(super) fn addresses(
    control: &binding_slot::BindingControlSet,
    source: &RouteScope,
    target: &ProjectContext,
    worker: &str,
    to: &str,
) -> Result<Addresses, Resp> {
    let sender = control
        .key_for_worker(&source.project_scope_id, &source.app_scope_id, worker)
        .map_err(Resp::err)?
        .cloned()
        .ok_or_else(|| Resp::err("IDENTITY_MAP_NOT_CURRENT: sender has no current name"))?;
    let recipient = control
        .key_for_worker(&target.project_scope, &target.app_scope_id, to)
        .map_err(Resp::err)?
        .cloned()
        .ok_or_else(|| Resp::err("IDENTITY_MAP_NOT_CURRENT: recipient has no current name"))?;
    Ok(Addresses { sender, recipient })
}

pub(super) fn reference(
    machine: &str,
    scope: &ProjectScopeId,
    app: &AppServerId,
    id: &str,
) -> MessageRef {
    MessageRef {
        machine: machine.to_owned(),
        project_scope: scope.clone(),
        app_scope_id: app.clone(),
        msg_id: crate::identity::MessageId::new(id).expect("committed message ID"),
    }
}

pub(super) fn encoded(reference: &MessageRef) -> String {
    crate::proto::format_collab_ref(&MessageReference::Message(reference.clone()))
        .expect("typed message reference serializes")
}

pub(super) fn validate_reply(
    control: &binding_slot::BindingControlSet,
    reply_addresses: &Addresses,
    source: &Server,
    state: &State,
    scope: &RouteScope,
    target: &Server,
    worker: &str,
    to: &str,
    reply: &str,
    machine: &str,
) -> Result<(), Resp> {
    let id = if reply.starts_with(crate::proto::COLLAB_REF_PREFIX) {
        let reference = crate::proto::parse_collab_ref(reply)
            .map_err(|error| Resp::err(format!("IN_REPLY_TO_REF_INVALID: {error}")))?;
        let MessageReference::Message(reference) = reference else {
            return Err(Resp::err(
                "IN_REPLY_TO_REF_INVALID: a message reference is required",
            ));
        };
        if !reference.machine.eq_ignore_ascii_case(machine)
            || reference.project_scope != scope.project_scope_id
            || reference.app_scope_id != scope.app_scope_id
        {
            return Err(Resp::err("IN_REPLY_TO_OWNER_MISMATCH: original must belong to the replying caller's project/app"));
        }
        reference.msg_id.to_string()
    } else {
        reply.to_owned()
    };
    if !message_id_is_path_safe(&id) {
        return Err(Resp::err("IN_REPLY_TO_REF_INVALID: unsafe message ID"));
    }
    let original = read_message_status_target(source, state, &id)
        .map_err(|error| Resp::err(format!("MESSAGE_LOOKUP_FAILED: {error}")))?
        .ok_or_else(|| Resp::err("IN_REPLY_TO_NOT_FOUND: original is absent from the replying caller's mailbox owner"))?;
    let valid = if let Some(addresses) = state.cross_project_addresses.get(&id) {
        addresses.recipient == reply_addresses.sender
            && addresses.sender == reply_addresses.recipient
            && holder(control, &addresses.recipient)? == Some(worker)
            && holder(control, &addresses.sender)? == Some(to)
    } else {
        // No historical name inference: legacy messages retain exact actor access.
        original.to == worker && original.from == format!("{to}@{}", target.root.display())
    };
    if !valid {
        return Err(Resp::err("IN_REPLY_TO_PARTICIPANT_MISMATCH: reply must reverse the exact original sender and recipient"));
    }
    Ok(())
}

/// Authenticate in the caller's project before reading the selected mailbox.
pub(super) fn status(
    manager: &ProjectRuntimeManager,
    caller: &Server,
    caller_app: &AppServerId,
    caller_runtime: Option<&crate::identity::RuntimeIdentity>,
    worker: &str,
    owner: &Server,
    reference: &MessageRef,
) -> Resp {
    // Same lock order as name replacement and cross-project sends.
    let _register = manager.register_gate.lock().unwrap();
    let control = match manager.binding_control_store().load() {
        Ok(control) => control,
        Err(error) => return Resp::err(error),
    };
    let caller_project = match GlobalState::canonical_project_scope(&caller.root) {
        Ok(project) => project,
        Err(error) => return Resp::err(error.to_string()),
    };
    let current = match control.current_for_worker(&caller_project, caller_app, worker) {
        Ok(Some(slot)) => slot.active_binding().expect("current slot has a binding"),
        Ok(None) => return Resp::err("IDENTITY_MAP_NOT_CURRENT: caller has no current name"),
        Err(error) => return Resp::err(error),
    };
    let binding = &current.binding;
    let registered = crate::identity::RuntimeIdentity {
        agent_id: binding.agent_id.clone(),
        runtime_id: binding.runtime_id.clone(),
        appserver_id: binding.app_scope_id.clone(),
        binding_id: binding.binding_id.clone(),
        session_id: binding.session_id.clone(),
        native_thread_id: binding.native_thread_id.clone(),
    };
    if caller_runtime != Some(&registered) {
        return Resp::err("IDENTITY_MAP_NOT_CURRENT: caller binding is no longer current");
    }
    let id = reference.msg_id.as_str();
    if !message_id_is_path_safe(id) {
        return Resp::err("MESSAGE_ID_INVALID: unsafe message ID");
    }
    let (message, addresses, mut response) = {
        let state = owner.state.lock().unwrap();
        let message = match read_message_status_target(owner, &state, id) {
            Ok(Some(message)) => message,
            Ok(None) => return Resp::err("MESSAGE_NOT_FOUND: message is absent from the referenced project mailbox; use message_ref returned by cross-project send"),
            Err(error) => return Resp::err(format!("MESSAGE_LOOKUP_FAILED: {error}")),
        };
        let local_caller = caller.root == owner.root && caller_app == &reference.app_scope_id;
        let addresses = state.cross_project_addresses.get(id).cloned();
        let mut permitted = if let Some(addresses) = &addresses {
            let check = |address: &binding_slot::IdentityBindingKey| -> Result<bool, Resp> {
                Ok(
                    address.project_scope.as_str() == caller.root.to_string_lossy()
                        && &address.app_scope_id == caller_app
                        && holder(&control, address)? == Some(worker),
                )
            };
            match check(&addresses.sender)
                .and_then(|sender| check(&addresses.recipient).map(|recipient| sender || recipient))
            {
                Ok(permitted) => permitted,
                Err(error) => return error,
            }
        } else {
            let sender = if message.from.contains('@') {
                message.from == format!("{worker}@{}", caller.root.display())
            } else {
                local_caller && message.from == worker
            };
            sender || (local_caller && message.to == worker)
        };
        // Same-project status uses the same open-obligation authority as the
        // direct status consumer. This does not infer legacy message names.
        if !permitted && local_caller {
            if let Some(obligation) = state.reply_obligations.get(id).filter(|obligation| {
                obligation.message_id == id
                    && obligation.project == reference.project_scope.as_str()
            }) {
                permitted = match reply_obligation::recipient_matches(
                    &control, caller_app, obligation, worker,
                ) {
                    Ok(permitted) => permitted,
                    Err(error) => return Resp::err(error),
                };
            }
        }
        if !permitted {
            return Resp::err("CALLER_NOT_PERMITTED: authenticated caller is neither the exact sender nor recipient");
        }
        let response = message_status_data(&state, message.clone(), state.answered(id));
        (message, addresses, response)
    };
    if let Some((sender, project)) = message.from.split_once('@') {
        response.data["answered"] = json!(false);
        // Only the original sender's reverse mailbox can contain a valid reply.
        let routes: Vec<_> = manager
            .routes
            .lock()
            .unwrap()
            .iter()
            .filter(|((app, scope), _)| {
                scope == project
                    && addresses
                        .as_ref()
                        .is_none_or(|addresses| app == addresses.sender.app_scope_id.as_str())
            })
            .map(|(key, route)| (key.clone(), route.root.clone(), route.storage_root.clone()))
            .collect();
        let qualified = encoded(reference);
        let expected_from = format!("{}@{}", message.to, owner.root.display());
        for (key, root, storage) in routes {
            let replies = match manager.ensure_runtime(&key, &root, &storage) {
                Ok(runtime) => runtime,
                Err(error) => {
                    return Resp::err(format!(
                        "MESSAGE_LOOKUP_FAILED: reply owner unavailable: {error}"
                    ))
                }
            };
            let state = replies.state.lock().unwrap();
            if state.msgs.values().any(|reply| {
                (if let Some(addresses) = &addresses {
                    state
                        .cross_project_addresses
                        .get(&reply.id)
                        .is_some_and(|reply_addresses| {
                            reply_addresses.sender == addresses.recipient
                                && reply_addresses.recipient == addresses.sender
                        })
                } else {
                    reply.to == sender && reply.from == expected_from
                }) && reply
                    .in_reply_to
                    .as_deref()
                    .is_some_and(|link| link == id || link == qualified)
            }) {
                response.data["answered"] = json!(true);
                break;
            }
        }
    }
    response.data["message_ref"] = json!(encoded(reference));
    response
}
