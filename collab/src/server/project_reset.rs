//! Online project reset preserves the host's current state and foreign scopes.
use super::*;
use std::collections::HashSet;
use std::io::Write;

pub(super) fn route_is_deinitialized(root: &Path) -> Result<bool, String> {
    match std::fs::metadata(root.join(".agent-collab")) {
        Ok(metadata) => Ok(!metadata.is_dir()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(format!(
            "STALE_ROUTE_INSPECTION_FAILED: {}: {e}",
            root.display()
        )),
    }
}

pub(super) fn retire_stale_project_routes(host: &Server, scope: &str) -> Result<(), String> {
    let bindings: Vec<RuntimeBinding> = {
        let state = host.state.lock().unwrap();
        state
            .global
            .current_thread_routes
            .values()
            .chain(state.global.legacy_thread_routes.values())
            .filter(|binding| binding.project_scope.as_str() == scope)
            .cloned()
            .collect()
    };
    for binding in bindings {
        let owns_binding = host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            .is_some();
        let events = if owns_binding {
            pane_reclaim_events("automatic-stale-route-cleanup", &binding)?
        } else {
            vec![Event::GlobalCurrentThreadRouteRetired { binding }]
        };
        host.commit_checked(&events)
            .map_err(|e| format!("STALE_ROUTE_CLEANUP_FAILED: {e}"))?;
    }
    Ok(())
}

/// The active DSH gateway workers owned by this project scope. Their session,
/// endpoint, binding and credential are host-owned recovery facts and survive
/// project reset; every other worker is disposable project state.
fn dsh_workers_for_scope(state: &State, scope: &str) -> Vec<String> {
    state
        .global
        .projects
        .get(scope)
        .into_iter()
        .flat_map(|project| project.runtime_bindings.values())
        .filter(|binding| {
            state
                .workers
                .get(binding.agent_id.as_str())
                .and_then(super::selected_transport_for_worker)
                .is_some_and(|transport| transport.kind == super::TransportKind::Dsh)
        })
        .map(|binding| binding.agent_id.as_str().to_owned())
        .collect()
}

/// Remove only this project's projections from the mixed resident reducer.
/// Project-local reducers have no foreign business records; the same worker
/// ownership filtering also handles mixed resident state.
pub(super) fn clear_project(
    state: &mut State,
    scope: &str,
    clear_local: bool,
    worker_ids: &[String],
) {
    // A live DSH gateway binding is host-owned recovery state, not project
    // business state. Reset keeps the current DSH session, communication API
    // endpoint and committed binding so the gateway receive loop can recover
    // after reset and daemon restart without reviving old tasks, messages or
    // grants.
    let dsh_workers = dsh_workers_for_scope(state, scope);
    let mut workers: HashSet<String> = state
        .global
        .projects
        .get(scope)
        .into_iter()
        .flat_map(|p| p.runtime_bindings.values())
        .map(|b| b.agent_id.as_str().to_owned())
        .collect();
    workers.extend(
        state
            .workers
            .values()
            .filter(|w| w.cwd == scope)
            .map(|w| w.id.clone()),
    );
    workers.extend(worker_ids.iter().cloned());
    // Agent ids can occur in independent project reducers. A host worker row
    // with another cwd is not owned by this reset merely because an external
    // project's binding happens to use the same agent id.
    workers.retain(|id| {
        clear_local
            || state
                .workers
                .get(id)
                .is_none_or(|worker| worker.cwd == scope || worker_ids.contains(id))
    });
    let mut foreign_workers: HashSet<String> = state
        .global
        .projects
        .iter()
        .filter(|(key, _)| key.as_str() != scope)
        .flat_map(|(_, p)| p.runtime_bindings.values())
        .map(|b| b.agent_id.as_str().to_owned())
        .collect();
    foreign_workers.extend(
        state
            .workers
            .values()
            .filter(|w| !workers.contains(&w.id))
            .map(|w| w.id.clone()),
    );
    let tasks: HashSet<String> = state
        .tasks
        .values()
        .filter(|task| {
            workers.contains(&task.owner)
                || workers.contains(&task.created_by)
                || (clear_local
                    && !foreign_workers.contains(&task.owner)
                    && !foreign_workers.contains(&task.created_by))
        })
        .map(|t| t.id.clone())
        .collect();
    let messages: Vec<String> = state
        .msgs
        .values()
        .filter(
            |message| match state.mailbox_name_addresses.get(&message.id) {
                Some(address) => address.project_scope == scope,
                None => {
                    workers.contains(&message.from)
                        || workers.contains(&message.to)
                        || (clear_local
                            && !foreign_workers.contains(&message.from)
                            && !foreign_workers.contains(&message.to))
                }
            },
        )
        .map(|m| m.id.clone())
        .collect();
    let dropped_messages: HashSet<String> = messages.iter().cloned().collect();
    for id in messages {
        state.drop_message(&id);
    }
    // Obligations are project-owned control state. A reset removes only this
    // scope's markers, plus any whose original message was just dropped.
    state.reply_obligations.retain(|message_id, obligation| {
        obligation.project != scope && !dropped_messages.contains(message_id)
    });
    state
        .conversation_defaults
        .retain(|_, default| default.project_scope != scope);
    let remaining_messages: HashSet<String> = state.msgs.keys().cloned().collect();
    state
        .channel_envelopes
        .retain(|message_id, _| remaining_messages.contains(message_id));
    state
        .platform_event_index
        .retain(|_, message_id| remaining_messages.contains(message_id));
    state
        .channel_reply_routes
        .retain(|message_id, _| remaining_messages.contains(message_id));
    state
        .channel_delivery_results
        .retain(|message_id, _| remaining_messages.contains(message_id));
    state.tasks.retain(|id, _| !tasks.contains(id));
    state.board_details.retain(|id, _| !tasks.contains(id));
    state.task_lifecycle.retain(|id, _| !tasks.contains(id));
    state.task_assignments.retain(|id, _| !tasks.contains(id));
    state.pending_merges.retain(|id, _| !tasks.contains(id));
    state
        .cleanup_receipts
        .retain(|_, receipt| !tasks.contains(&receipt.task_id));
    state
        .scheduler_admissions
        .retain(|_, r| !tasks.contains(&r.task_id) && !workers.contains(&r.worker_id));
    state
        .receive_receipts
        .retain(|_, r| !workers.contains(&r.worker_id));
    state
        .notification_subscriptions
        .retain(|_, s| !workers.contains(&s.worker_id));
    state.keepalives.retain(|id, _| !workers.contains(id));
    state.worker_snapshots.retain(|id, _| !workers.contains(id));
    state.worker_closures.retain(|id, _| !workers.contains(id));
    let subagents: HashSet<String> = state
        .subagents
        .values()
        .filter(|s| workers.contains(&s.peer))
        .map(|s| s.id.clone())
        .collect();
    state.subagents.retain(|id, _| !subagents.contains(id));
    state
        .subagent_snapshots
        .retain(|id, _| !subagents.contains(id));
    state
        .responsibility_fences
        .retain(|_, f| f.project_scope != scope);
    state
        .peer_lifecycle_operations
        .retain(|_, op| op.project_scope.as_str() != scope);
    state
        .worktree_bindings
        .retain(|_, b| b.owning_project_scope != scope);
    state.name_occupancies.remove(scope);
    state.name_occupancy_revisions.remove(scope);
    state.name_work_roles.remove(scope);
    state.workflow_templates.remove(scope);
    state.agent_presence.remove(scope);
    state.agent_activity.remove(scope);
    state.delivery_modes.retain(|id, _| !workers.contains(id));
    state
        .delivery_source_threads
        .retain(|id, _| !workers.contains(id));
    state.wake_bindings.retain(|id, _| !workers.contains(id));
    // Keep the live DSH gateway worker record (token, transport, cwd) so the
    // receive loop can re-enroll without Register. Its tasks, messages and
    // subscriptions above are still removed because they are project business
    // state; only the identity record itself is host-owned recovery material.
    state
        .workers
        .retain(|id, _| !workers.contains(id) || dsh_workers.contains(id));
    if state
        .master_worker_id
        .as_ref()
        .is_some_and(|id| workers.contains(id))
    {
        state.master_worker_id = None;
        state.master_assigned_by = None;
        state.master_approval = None;
        state.master_assigned_ms = None;
    }
    if clear_local {
        state.project_work_policy.clear();
        state.master_wake = Default::default();
    }
    // Registration receipts already carry their typed command.binding scope.
    // Unscoped legacy receipts belong to this reducer's local project.
    let retain_receipt = |outcome: &serde_json::Value| match outcome
        .get("command")
        .and_then(|command| command.get("binding"))
        .and_then(|binding| binding.get("project_scope"))
        .and_then(|scope| scope.as_str())
    {
        Some(owner) => owner != scope,
        None => !clear_local,
    };
    state
        .global
        .command_receipts
        .retain(|_, receipt| retain_receipt(&receipt.outcome));
    // Keep the retained DSH project registration and its committed runtime
    // binding. The reducer is the authoritative source of the worker token and
    // transport; dropping it would leave the control slot pointing at a missing
    // runtime and make recovery fail closed on startup.
    let retained_dsh_app_scopes: HashSet<String> = state
        .global
        .projects
        .get(scope)
        .into_iter()
        .flat_map(|project| {
            project
                .runtime_bindings
                .values()
                .filter(|binding| dsh_workers.contains(&binding.agent_id.as_str().to_owned()))
                .map(|binding| binding.app_scope_id.as_str().to_owned())
        })
        .collect();
    if let Some(project) = state.global.projects.get_mut(scope) {
        project.registrations.retain(|_, registration| {
            retained_dsh_app_scopes.contains(registration.app_scope_id.as_str())
        });
        project
            .runtime_bindings
            .retain(|_, binding| dsh_workers.contains(&binding.agent_id.as_str().to_owned()));
        project.master_grants.clear();
        project.bootstrap_policies.clear();
        if project.registrations.is_empty() && project.runtime_bindings.is_empty() {
            state.global.projects.remove(scope);
        }
    } else {
        state.global.projects.remove(scope);
    }
    state.global.current_thread_routes.retain(|_, b| {
        b.project_scope.as_str() != scope || dsh_workers.contains(&b.agent_id.as_str().to_owned())
    });
    state.global.legacy_thread_routes.retain(|_, b| {
        b.project_scope.as_str() != scope || dsh_workers.contains(&b.agent_id.as_str().to_owned())
    });
    state
        .global
        .current_thread_route_tombstones
        .retain(|_, b| b.project_scope.as_str() != scope);
}

/// Remove the route and its scope's control slots through the shared owner.
pub(super) fn delete_route(path: &Path, record: &HostRouteRecord) -> Result<(), String> {
    // Remove the route line and the scope's control slots in one shared
    // commit so a retained binding cannot resurrect the retired identity.
    let store = crate::server::binding_store::BindingControlStore::new(
        path.parent().ok_or("HOST_ROUTE_PATH_INVALID")?,
    );
    let routes = super::load_host_route_records(path)?;
    let mut control = store.load()?;
    let retained: Vec<HostRouteRecord> = routes
        .into_iter()
        .filter(|retained| {
            retained.project_scope != record.project_scope
                || retained.app_scope_id != record.app_scope_id
        })
        .collect();
    control.remove_scope(record.project_scope.as_str());
    store.save(&control)?.require_confirmed_route()?;
    crate::server::binding_store::publish_route_records(path, &retained)?.require_confirmed_route()
}

/// Remove this project's route records while retaining the route records for
/// app scopes that still own a live DSH gateway binding. This is one shared
/// control transaction so a retained route and its control slot cannot be
/// published independently.
pub(super) fn delete_project_routes_except_dsh(
    path: &Path,
    scope: &str,
    retained_app_scopes: &HashSet<String>,
) -> Result<(), String> {
    let store = crate::server::binding_store::BindingControlStore::new(
        path.parent().ok_or("HOST_ROUTE_PATH_INVALID")?,
    );
    let routes = super::load_host_route_records(path)?;
    let mut control = store.load()?;
    let retained = routes
        .into_iter()
        .filter(|route| {
            route.project_scope != scope || retained_app_scopes.contains(&route.app_scope_id)
        })
        .collect::<Vec<_>>();
    control.remove_scope_except_dsh(scope);
    store.save(&control)?.require_confirmed_route()?;
    crate::server::binding_store::publish_route_records(path, &retained)?.require_confirmed_route()
}

/// Delete project data, keeping only exact live host resources when the
/// resident host state/lock happens to live inside the project tree.
pub(super) fn remove_project_tree(path: &Path, preserved: &[PathBuf]) -> Result<(), String> {
    if preserved.iter().any(|keep| keep == path) {
        return Ok(());
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("inspect {}: {e}", path.display())),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        for child in std::fs::read_dir(path).map_err(|e| format!("list {}: {e}", path.display()))? {
            let child = child.map_err(|e| e.to_string())?;
            remove_project_tree(&child.path(), preserved)?;
        }
        if !preserved.iter().any(|keep| keep.starts_with(path)) {
            std::fs::remove_dir(path).map_err(|e| format!("remove {}: {e}", path.display()))?;
            sync_parent_dir(path)
                .map_err(|e| format!("persist removal {}: {e}", path.display()))?;
        }
    } else {
        std::fs::remove_file(path).map_err(|e| format!("remove {}: {e}", path.display()))?;
        sync_parent_dir(path).map_err(|e| format!("persist removal {}: {e}", path.display()))?;
    }
    Ok(())
}

pub(super) fn finish_reset(host: &Server, route_file: &Path, scope: &str) -> Result<(), String> {
    let store =
        crate::server::binding_store::BindingControlStore::new(host.host_paths.state_root());
    // This also upgrades an interrupted legacy reset before cleanup. The
    // current map is retired first on normal execution and startup alike.
    store.begin_reset(scope)?;
    if !host
        .state
        .lock()
        .unwrap()
        .pending_project_resets
        .contains_key(scope)
    {
        let worker_ids = host
            .state
            .lock()
            .unwrap()
            .workers
            .values()
            .filter(|worker| worker.cwd == scope)
            .map(|worker| worker.id.clone())
            .collect();
        host.commit_checked(&[Event::ProjectResetStarted {
            project_scope: scope.to_owned(),
            clear_local: host.root == Path::new(scope),
            worker_ids,
        }])
        .map_err(|error| format!("persist reset cleanup: {error}"))?;
    }
    let project_records = load_host_route_records(route_file)?
        .into_iter()
        .filter(|record| record.project_scope == scope)
        .collect::<Vec<_>>();
    // The active DSH gateway identity is host-owned recovery material. Reset
    // keeps its worker/token/transport so `dsh_rebind` can re-enroll the
    // receive loop after reset and daemon restart without Register. Other
    // identities in the scope are disposable project state.
    let retained_dsh_workers = {
        let state = host.state.lock().unwrap();
        dsh_workers_for_scope(&state, scope)
    };
    let retained_app_scopes: HashSet<String> = {
        let state = host.state.lock().unwrap();
        state
            .global
            .projects
            .get(scope)
            .into_iter()
            .flat_map(|project| {
                project
                    .runtime_bindings
                    .values()
                    .filter(|binding| {
                        retained_dsh_workers.contains(&binding.agent_id.as_str().to_owned())
                    })
                    .map(|binding| binding.app_scope_id.as_str().to_owned())
            })
            .collect()
    };
    remove_host_identities(&host.host_paths, scope, &retained_dsh_workers)?;
    remove_archived_identities(&host.host_paths, scope)?;
    let preserved = [
        host.state_path.clone(),
        host.host_paths.state_root().to_path_buf(),
        host.root.join(".agent-collab/server/daemon.lock"),
    ]
    .iter()
    .map(|path| storage_owner_path(path))
    .collect::<Result<Vec<_>, _>>()?;
    remove_project_tree(&Path::new(scope).join(".agent-collab"), &preserved)?;
    remove_project_tree(&Path::new(scope).join(".agent-collab-v2"), &preserved)?;
    // A retained DSH route must stay replayable after daemon restart:
    // startup pruning treats any project root without a `.agent-collab`
    // directory as deinitialized and retires its route. Keep the project
    // marker (no business data) for every app scope that still owns a live
    // DSH gateway binding.
    if !retained_app_scopes.is_empty() {
        std::fs::create_dir_all(Path::new(scope).join(".agent-collab"))
            .map_err(|e| format!("persist retained project marker: {e}"))?;
    }
    for record in &project_records {
        // Delete the central app storage for every route record, including
        // retained DSH app scopes. The DSH route, control slot and identity are
        // host-owned recovery facts kept elsewhere; the retained business
        // business state under central storage must not survive reset.
        let central = host
            .host_paths
            .app_storage_root(scope, &record.app_scope_id);
        let central = storage_owner_path(&central)?;
        remove_project_tree(&central, &preserved)?;
    }
    // Route records are the durable discovery source for app-scoped central
    // storage. Retain the active DSH route/control binding; retire every other
    // route for the scope in one shared transaction.
    delete_project_routes_except_dsh(route_file, scope, &retained_app_scopes)?;
    host.commit_checked(&[Event::ProjectResetCompleted {
        project_scope: scope.to_owned(),
    }])
    .map_err(|e| format!("reset completion durability: {e}"))?;
    store.complete_reset(scope)?;
    Ok(())
}

fn remove_archived_identities(paths: &HostPaths, scope: &str) -> Result<(), String> {
    let entries = match std::fs::read_dir(paths.state_root().join("archives")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("list retired identities: {e}")),
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("identities-retired-")
        {
            continue;
        }
        for worker in std::fs::read_dir(entry.path()).map_err(|e| e.to_string())? {
            let worker = worker.map_err(|e| e.to_string())?;
            let path = worker.path().join("identity.json");
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("read retired identity: {e}")),
            };
            let identity: crate::identity::Identity =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if identity
                .project_scope
                .as_ref()
                .is_some_and(|p| p.as_str() == scope)
            {
                remove_project_tree(&worker.path(), &[])?;
            }
        }
    }
    Ok(())
}

fn remove_host_identities(
    paths: &HostPaths,
    scope: &str,
    retained_workers: &[String],
) -> Result<(), String> {
    let root = paths.state_root().join("identities");
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("list host identities: {e}")),
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let worker = entry
            .file_name()
            .into_string()
            .map_err(|_| "invalid host identity path")?;
        let identity = crate::identity::read_persisted(paths, &worker)
            .map_err(|e| format!("read host identity {worker}: {e}"))?;
        if identity.is_some_and(|identity| {
            identity
                .project_scope
                .as_ref()
                .is_some_and(|p| p.as_str() == scope)
                && !retained_workers.contains(&worker)
        }) {
            remove_project_tree(&entry.path(), &[])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
