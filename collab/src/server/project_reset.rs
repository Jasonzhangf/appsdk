//! Online project reset. The host journal is never removed.
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

/// Remove only this project's projections from the mixed resident reducer.
/// Project-local reducers have no foreign business records; the same worker
/// ownership filtering also handles legacy mixed resident journals.
pub(super) fn clear_project(
    state: &mut State,
    scope: &str,
    clear_local: bool,
    worker_ids: &[String],
) {
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
        .filter(|message| {
            workers.contains(&message.from)
                || workers.contains(&message.to)
                || (clear_local
                    && !foreign_workers.contains(&message.from)
                    && !foreign_workers.contains(&message.to))
        })
        .map(|m| m.id.clone())
        .collect();
    for id in messages {
        state.drop_message(&id);
    }
    state.tasks.retain(|id, _| !tasks.contains(id));
    state.board_details.retain(|id, _| !tasks.contains(id));
    state.task_lifecycle.retain(|id, _| !tasks.contains(id));
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
    state.worker_names.retain(|id, _| !workers.contains(id));
    state.delivery_modes.retain(|id, _| !workers.contains(id));
    state
        .delivery_source_threads
        .retain(|id, _| !workers.contains(id));
    state.wake_bindings.retain(|id, _| !workers.contains(id));
    state.workers.retain(|id, _| !workers.contains(id));
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
        state.master_wake = Default::default();
        state.migration = None;
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
        .command_receipts
        .retain(|_, receipt| retain_receipt(&receipt.outcome));
    state
        .global
        .command_receipts
        .retain(|_, receipt| retain_receipt(&receipt.outcome));
    state.global.projects.remove(scope);
    state
        .global
        .current_thread_routes
        .retain(|_, b| b.project_scope.as_str() != scope);
    state
        .global
        .legacy_thread_routes
        .retain(|_, b| b.project_scope.as_str() != scope);
    state
        .global
        .current_thread_route_tombstones
        .retain(|_, b| b.project_scope.as_str() != scope);
    state
        .global
        .master_grant_replacement_intents
        .retain(|_, r| r.project_scope.as_str() != scope);
    state
        .global
        .master_grant_replacement_receipts
        .retain(|_, r| r.project_scope.as_str() != scope);
}

/// Append deletion through the live route owner. Never rewrite a live journal.
pub(super) fn delete_route(path: &Path, record: &HostRouteRecord) -> Result<(), String> {
    // Called under the manager's registration gate (or before startup
    // admission). Remove the historical bytes as well as the active index.
    let records = load_host_route_records(path)?;
    let mut body = Vec::new();
    for retained in records.into_iter().filter(|retained| {
        retained.project_scope != record.project_scope
            || retained.app_scope_id != record.app_scope_id
    }) {
        serde_json::to_writer(&mut body, &retained).map_err(|e| e.to_string())?;
        body.push(b'\n');
    }
    let temporary = path.with_extension("reset-tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|e| format!("route delete open {}: {e}", path.display()))?;
    file.write_all(&body)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("route delete persist {}: {e}", path.display()))?;
    std::fs::rename(&temporary, path).map_err(|e| format!("route delete publish: {e}"))?;
    sync_parent_dir(path).map_err(|e| format!("route delete directory: {e}"))
}

/// Delete project data, keeping only exact live host resources when the
/// resident host journal/lock happens to live inside the project tree.
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

pub(super) fn finish_reset(host: &Server, route_journal: &Path, scope: &str) -> Result<(), String> {
    for record in load_host_route_records(route_journal)? {
        if record.project_scope == scope {
            delete_route(route_journal, &record)?;
        }
    }
    remove_host_identities(&host.host_paths, scope)?;
    remove_archived_identities(&host.host_paths, scope)?;
    crate::context_operation::remove_project_proofs(&host.host_paths, scope)
        .map_err(|e| format!("remove project operation proofs: {e}"))?;
    let preserved = [
        host.journal_path.clone(),
        host.host_paths.state_root().to_path_buf(),
        host.root.join(".agent-collab/server/daemon.lock"),
    ]
    .iter()
    .map(|path| storage_owner_path(path))
    .collect::<Result<Vec<_>, _>>()?;
    remove_project_tree(&Path::new(scope).join(".agent-collab"), &preserved)?;
    remove_project_tree(&Path::new(scope).join(".agent-collab-v2"), &preserved)?;
    // The resident journal mixes projects. Compact its cleared reducer under
    // the writer's state lock, retaining the pending intent until completion.
    {
        let state = host.state.lock().unwrap();
        host.rewrite_journal_locked(&state)
            .map_err(|e| format!("erase project journal history: {e}"))?;
    }
    host.commit_checked(&[Event::ProjectResetCompleted {
        project_scope: scope.to_owned(),
    }])
    .map_err(|e| format!("reset completion durability: {e}"))?;
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

fn remove_host_identities(paths: &HostPaths, scope: &str) -> Result<(), String> {
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
        }) {
            remove_project_tree(&entry.path(), &[])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
