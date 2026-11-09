//! Federation-aware conversation-default setter.
//!
//! The local setter in `part_10.rs` remains the sole owner of local defaults.
//! This module only adds the remote branch. It classifies the supplied target
//! through the crate address owner, verifies the caller's current local
//! identity, asks the declared target machine for its authoritative directory,
//! then commits the canonical remote classification in one local transaction.
//!
//! A remote default stores only the declared machine and the canonical local
//! project/name. It never stores a remote worker/session id, never copies a
//! peer token, and never points the local QQ receiver at a remote project
//! socket.

use super::*;
use crate::address::ResolvedAddress;
use crate::federation::FederationClient;
use crate::proto::{
    AddressInput, DirectoryTarget, FederationReq, FederationResponse, LocalNameAddress,
};

const REMOTE_DEFAULT_DEADLINE: Duration = Duration::from_secs(15);

/// Dispatch `Req::DefaultSet`. Local candidates stay with the existing local
/// setter. Remote candidates are verified against the declared target
/// machine's authoritative directory before the old default can change.
pub(super) async fn dispatch(
    manager: Arc<ProjectRuntimeManager>,
    context: Option<ProjectContext>,
    request: Req,
) -> (Arc<Server>, Resp) {
    let host = manager.host.clone();
    let (worker_id, token, channel, channel_instance, conversation, target_address) = match &request
    {
        Req::DefaultSet {
            worker_id,
            token,
            channel,
            channel_instance,
            conversation,
            target_address,
        } => (
            worker_id.clone(),
            token.clone(),
            channel.clone(),
            channel_instance.clone(),
            conversation.clone(),
            target_address.clone(),
        ),
        _ => return (host, Resp::err("FEDERATION_DEFAULT_REQUEST_INVALID")),
    };
    // Keep the exact channel boundaries owned by the existing local setter.
    if let Err(error) = validate_default_scope(&channel, &channel_instance, &conversation) {
        return (host, Resp::err(error));
    }
    // Keep the generic project-route admission order that `DefaultSet` used
    // before the dedicated entry point: pending scope effects are recovered
    // before the request can mutate a conversation default.
    if let Some(context) = context.as_ref() {
        let recovery_manager = manager.clone();
        let recovery_context = context.clone();
        match tokio::task::spawn_blocking(move || {
            recovery_manager.recover_pending_effects_for_scope(&recovery_context)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return (host, Resp::err(error)),
            Err(error) => return (host, Resp::err(format!("handler join error: {error}"))),
        }
    }
    let resolved = match super::federation_runtime::resolve(
        &manager,
        &AddressInput::Legacy(target_address.clone()),
    ) {
        Ok(resolved) => resolved,
        Err(error) => {
            return (
                host,
                Resp::err(format!("DEFAULT_TARGET_UNRESOLVED: {error}")),
            );
        }
    };
    let ResolvedAddress::Remote { machine, target } = resolved else {
        // Local and legacy targets remain owned by the existing local setter.
        return match tokio::task::spawn_blocking(move || manager.dispatch_sync(context, request))
            .await
        {
            Ok(result) => result,
            Err(error) => (host, Resp::err(format!("handler join error: {error}"))),
        };
    };
    match dispatch_remote(
        manager,
        context,
        &request,
        &worker_id,
        &token,
        &channel,
        &channel_instance,
        &conversation,
        &target_address,
        machine,
        target,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => (host, Resp::err(error)),
    }
}

/// Reuse the local setter's declared default key boundaries before any network
/// work. A channel adapter that cannot own a QQ conversation must not be able
/// to create a remote default either.
fn validate_default_scope(
    channel: &str,
    channel_instance: &str,
    conversation: &str,
) -> Result<(), String> {
    if channel != "qqbot" {
        return Err(
            "UNSUPPORTED_CHANNEL: only the qqbot channel owns conversation defaults".into(),
        );
    }
    if channel_instance.trim().is_empty() {
        return Err(
            "CHANNEL_INSTANCE_REQUIRED: a default is owned by one exact channel instance".into(),
        );
    }
    if conversation.trim().is_empty() {
        return Err("CONVERSATION_REQUIRED: a nonempty conversation key is required".into());
    }
    Ok(())
}

struct LocalDefaultIdentity {
    route: RouteScope,
    name: String,
}

/// Read the caller's current local identity: registered worker, matching
/// token, current route scope and current stable-name map entry. This is the
/// same authority family used by local send/default admission, without
/// inventing a second token or identity store.
fn current_default_identity(
    server: &Arc<Server>,
    state: &State,
    worker_id: &str,
    token: &str,
) -> Result<LocalDefaultIdentity, String> {
    if state.admission_frozen() {
        return Err("PROJECT_RESET_PENDING".into());
    }
    let worker = state
        .workers
        .get(worker_id)
        .ok_or_else(|| format!("worker {worker_id} not registered"))?;
    if worker.token != token {
        return Err("token mismatch: identity does not own this worker_id".into());
    }
    let route = server_route_scope(server, state)?.ok_or_else(|| {
        "DEFAULT_PROJECT_SCOPE_MISSING: the current project route is unavailable".to_string()
    })?;
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load()?;
    let slot = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, worker_id)?
        .ok_or_else(|| {
            format!("IDENTITY_MAP_NOT_CURRENT: worker {worker_id} has no current stable name")
        })?;
    Ok(LocalDefaultIdentity {
        route,
        name: slot.identity_key.name.clone(),
    })
}

/// Machine-checked remote default. The caller identity is checked before the
/// network, the target directory is fetched outside every state lock, and the
/// caller identity plus current name are rechecked in the same state
/// transaction that commits the new default.
#[allow(clippy::too_many_arguments)]
async fn dispatch_remote(
    manager: Arc<ProjectRuntimeManager>,
    context: Option<ProjectContext>,
    request: &Req,
    worker_id: &str,
    token: &str,
    channel: &str,
    channel_instance: &str,
    conversation: &str,
    target_address: &str,
    machine: String,
    target: LocalNameAddress,
) -> Result<(Arc<Server>, Resp), String> {
    let context = context.ok_or("PROJECT_CONTEXT_REQUIRED")?;
    let server = manager.select_runtime(&context)?;
    validate_request_context(&server, request, Some(&context))?;
    let before = {
        let state = server.state.lock().unwrap();
        current_default_identity(&server, &state, worker_id, token)?
    };
    let configuration = manager
        .host
        .config
        .federation
        .clone()
        .ok_or("FEDERATION_NOT_CONFIGURED")?;
    let response = FederationClient::call(
        &configuration,
        &machine,
        &FederationReq::Directory {
            project: Some(target.project.clone()),
        },
        REMOTE_DEFAULT_DEADLINE,
    )
    .await
    .map_err(|error| error.to_string())?;
    let cleanup_error = response.cleanup_error;
    let targets = match response.response {
        FederationResponse::Directory { targets } => targets,
        FederationResponse::Error { code, detail } => return Err(format!("{code}: {detail}")),
        _ => return Err("FEDERATION_PROTOCOL_INVALID: expected a directory response".into()),
    };
    let canonical = unique_remote_target(&targets, &machine, &target)?;
    let key = crate::server::conversation_default::conversation_default_key(
        channel,
        channel_instance,
        conversation,
    );
    let mut state = server.state.lock().unwrap();
    let after = current_default_identity(&server, &state, worker_id, token)?;
    if after.route != before.route || after.name != before.name {
        return Err(
            "IDENTITY_CHANGED_DURING_DEFAULT_SET: the caller's current identity changed while the remote target was verified"
                .into(),
        );
    }
    let revision = state
        .conversation_defaults
        .get(&key)
        .map(|default| default.revision)
        .unwrap_or(0);
    let default = crate::server::conversation_default::ConversationDefault {
        channel: channel.to_owned(),
        channel_instance: channel_instance.to_owned(),
        conversation: conversation.to_owned(),
        project_scope: after.route.project_scope_id.as_str().to_owned(),
        target_address: target_address.to_owned(),
        resolved_target: Some(ResolvedAddress::Remote {
            machine,
            target: canonical,
        }),
        revision: revision + 1,
        updated_ms: now_ms(),
    };
    if let Err(error) = server.commit_locked(
        &mut state,
        &[Event::ConversationDefaultSet {
            default: default.clone(),
        }],
    ) {
        return Err(format!("DEFAULT_DURABILITY_FAILED: {error}"));
    }
    drop(state);
    Ok((
        server,
        super::federation_runtime::with_transport_error(
            Resp::data(json!({ "default": default })),
            cleanup_error,
        ),
    ))
}

/// Select the one directory entry that proves the requested project/name is a
/// current route on the declared machine. The remote `project_root` is display
/// data only; this function never uses it to choose a local socket.
fn unique_remote_target(
    targets: &[DirectoryTarget],
    machine: &str,
    requested: &LocalNameAddress,
) -> Result<LocalNameAddress, String> {
    let mut matches = targets.iter().filter(|target| {
        target
            .project
            .as_deref()
            .is_some_and(|project| project.eq_ignore_ascii_case(&requested.project))
            && target.display_name.eq_ignore_ascii_case(&requested.name)
            && target
                .machine
                .as_deref()
                .is_some_and(|declared| declared.eq_ignore_ascii_case(machine))
            && target.route.as_ref().is_some_and(|route| {
                route.machine.eq_ignore_ascii_case(machine)
                    && route.local.project.eq_ignore_ascii_case(&requested.project)
                    && route.local.name.eq_ignore_ascii_case(&requested.name)
            })
    });
    let Some(target) = matches.next() else {
        return Err(
            "DEFAULT_TARGET_UNRESOLVED: the remote directory has no current project/name holder on the declared machine"
                .into(),
        );
    };
    if matches.next().is_some() {
        return Err(
            "DEFAULT_TARGET_AMBIGUOUS: the remote directory has multiple current holders for this project/name"
                .into(),
        );
    }
    Ok(LocalNameAddress {
        project: target.project.clone().ok_or(
            "DEFAULT_TARGET_UNRESOLVED: the remote directory target has no canonical project",
        )?,
        name: target.display_name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::QualifiedNameAddress;

    fn remote_target(project: &str, name: &str, machine: &str) -> DirectoryTarget {
        DirectoryTarget {
            worker_id: format!("{machine}-{project}-{name}"),
            display_name: name.into(),
            session_id: None,
            app_scope: Some("cli".into()),
            transport: None,
            identity_valid: Some(true),
            endpoint_live: Some(true),
            project: Some(project.into()),
            address: Some(format!("{project}.{name}")),
            project_root: Some(format!("/remote/{project}")),
            channel_type: None,
            deliverable: Some("unknown".into()),
            machine: Some(machine.into()),
            qualified_address: Some(format!("{machine}.{project}.{name}")),
            route: Some(QualifiedNameAddress {
                machine: machine.into(),
                local: LocalNameAddress {
                    project: project.into(),
                    name: name.into(),
                },
            }),
        }
    }

    #[test]
    fn default_scope_keeps_the_local_setter_channel_boundaries() {
        assert!(validate_default_scope("qqbot", "qqbot@host", "u1:session").is_ok());
        assert!(validate_default_scope("slack", "qqbot@host", "u1:session")
            .unwrap_err()
            .contains("UNSUPPORTED_CHANNEL"));
        assert!(validate_default_scope("qqbot", " ", "u1:session")
            .unwrap_err()
            .contains("CHANNEL_INSTANCE_REQUIRED"));
        assert!(validate_default_scope("qqbot", "qqbot@host", " ")
            .unwrap_err()
            .contains("CONVERSATION_REQUIRED"));
    }

    #[test]
    fn current_default_identity_rejects_reset_started_during_remote_lookup() {
        let (server, root) = super::super::peer_tests::test_server();
        assert!(
            super::super::peer_tests::register_synthetic_peer(
                &server,
                "default-owner",
                "default-owner-thread",
            )
            .ok
        );
        let server = Arc::new(server);
        let mut state = server.state.lock().unwrap();
        let token = state.workers["default-owner"].token.clone();
        assert!(current_default_identity(&server, &state, "default-owner", &token).is_ok());
        state.pending_project_resets.insert("project".into(), true);
        assert_eq!(
            current_default_identity(&server, &state, "default-owner", &token)
                .err()
                .as_deref(),
            Some("PROJECT_RESET_PENDING"),
        );
        drop(state);
        drop(server);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remote_directory_requires_one_current_route_on_the_declared_machine() {
        let requested = LocalNameAddress {
            project: "appsdk".into(),
            name: "Alice".into(),
        };
        assert_eq!(
            unique_remote_target(
                &[remote_target("appsdk", "Alice", "macbookair")],
                "macbookair",
                &requested,
            )
            .unwrap(),
            requested,
        );
        assert!(unique_remote_target(&[], "macbookair", &requested).is_err());
        assert!(unique_remote_target(
            &[remote_target("appsdk", "Alice", "coder1")],
            "macbookair",
            &requested,
        )
        .is_err());
        assert!(unique_remote_target(
            &[
                remote_target("appsdk", "Alice", "macbookair"),
                remote_target("appsdk", "Alice", "macbookair"),
            ],
            "macbookair",
            &requested,
        )
        .is_err());
    }

    #[test]
    fn failed_remote_selection_does_not_rewrite_the_stored_default() {
        let mut state = State::default();
        let existing = crate::server::conversation_default::ConversationDefault {
            channel: "qqbot".into(),
            channel_instance: "qqbot@host".into(),
            conversation: "u1:session".into(),
            project_scope: "/project".into(),
            target_address: "appsdk.Alice".into(),
            resolved_target: None,
            revision: 3,
            updated_ms: 1,
        };
        state
            .conversation_defaults
            .insert(existing.key(), existing.clone());
        assert!(unique_remote_target(
            &[],
            "macbookair",
            &LocalNameAddress {
                project: "appsdk".into(),
                name: "Alice".into(),
            },
        )
        .is_err());
        assert_eq!(
            crate::server::conversation_default::lookup_default(
                &state,
                "qqbot",
                "qqbot@host",
                "u1:session",
            ),
            Some(&existing),
        );
    }
}
