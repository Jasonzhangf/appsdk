fn validate_transport_candidates(
    server: &Server,
    candidates: &TransportCandidates,
    registration_cwd: &str,
) -> Result<SelectedTransport, String> {
    let requested_root = std::fs::canonicalize(registration_cwd)
        .map_err(|error| format!("RUNTIME_BINDING_REJECTED: registration cwd: {error}"))?;
    let candidate_root = std::fs::canonicalize(&server.root)
        .map_err(|error| format!("RUNTIME_BINDING_REJECTED: project root: {error}"))?;
    if candidate_root != requested_root {
        return Err(format!(
            "RUNTIME_BINDING_REJECTED: registration cwd {} does not match project root {}",
            requested_root.display(),
            candidate_root.display()
        ));
    }
    // dsh is a mutually exclusive channel: it has no pane to act as an App
    // Server recovery anchor, and a caller supplying dsh *and* a tmux/appserver
    // candidate has ambiguous intent. Resolving that silently is exactly the
    // candidate-shadowing this design forbids, so the ambiguous set is refused
    // rather than ranked.
    if candidates.dsh.is_some() && (candidates.appserver.is_some() || candidates.tmux.is_some()) {
        return Err(
            "DSH_ENDPOINT_REJECTED: a dsh candidate must not be combined with an App Server or tmux candidate"
                .into(),
        );
    }
    if let Some(dsh) = candidates.dsh.as_ref() {
        return admit_dsh_candidate(dsh, &candidate_root);
    }
    if let Some(appserver) = candidates.appserver.as_ref() {
        let app_cwd = std::fs::canonicalize(&appserver.cwd).map_err(|error| {
            format!("RUNTIME_BINDING_REJECTED: App Server candidate cwd: {error}")
        })?;
        if app_cwd != candidate_root {
            return Err(format!(
                "RUNTIME_BINDING_REJECTED: App Server candidate cwd {} does not match project root {}",
                app_cwd.display(),
                candidate_root.display()
            ));
        }
        let mut selected = crate::client::adapters::verify_candidate(appserver)
            .map_err(|error| format!("APPSERVER_ENDPOINT_REJECTED: {error}"))?;
        if let Some(tmux) = candidates.tmux.as_ref() {
            if !tmux.cwd.starts_with('/') {
                return Err("RUNTIME_BINDING_REJECTED: tmux candidate cwd must be absolute".into());
            }
            match crate::client::adapters::tmux::probe(&tmux.endpoint)? {
                crate::client::adapters::tmux::PanePresence::Present => {}
                crate::client::adapters::tmux::PanePresence::Missing => {
                    return Err("TMUX_PANE_MISSING: recovery pane is not live".into())
                }
                crate::client::adapters::tmux::PanePresence::Unknown => {
                    return Err("TMUX_PANE_UNKNOWN: recovery pane liveness is uncertain".into())
                }
            }
            let mut recovery = tmux.endpoint.clone();
            if recovery.codex_session_id.is_none() {
                recovery.codex_session_id = Some(appserver.session_id.clone());
            }
            if recovery.codex_thread_id.is_none() {
                recovery.codex_thread_id = Some(appserver.thread_id.clone());
            }
            selected.tmux_endpoint = Some(recovery);
            if !selected
                .capabilities
                .iter()
                .any(|cap| cap == "pane_recovery_anchor")
            {
                selected.capabilities.push("pane_recovery_anchor".into());
            }
        }
        return Ok(selected);
    }
    if let Some(candidate) = candidates.tmux.as_ref() {
        if !candidate.cwd.starts_with('/') {
            return Err("RUNTIME_BINDING_REJECTED: tmux candidate cwd must be absolute".into());
        }
        match crate::client::adapters::tmux::probe(&candidate.endpoint)? {
            crate::client::adapters::tmux::PanePresence::Present => {}
            crate::client::adapters::tmux::PanePresence::Missing => {
                return Err("TMUX_PANE_MISSING: registration pane is not live".into())
            }
            crate::client::adapters::tmux::PanePresence::Unknown => {
                return Err("TMUX_PANE_UNKNOWN: registration pane liveness is uncertain".into())
            }
        }
        if candidate
            .endpoint
            .codex_session_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
            || candidate
                .endpoint
                .codex_thread_id
                .as_deref()
                .is_some_and(|id| id.trim().is_empty())
        {
            return Err("TMUX_IDENTITY_INVALID: empty Codex identity anchor".into());
        }
        return Ok(SelectedTransport {
            kind: TransportKind::Tmux,
            endpoint: Some(candidate.endpoint.socket_path.clone()),
            namespace: Some(candidate.endpoint.tmux_session_id.clone()),
            session_id: candidate
                .endpoint
                .codex_session_id
                .clone()
                .or_else(|| Some(candidate.endpoint.tmux_session_id.clone())),
            thread_id: candidate
                .endpoint
                .codex_thread_id
                .clone()
                .or_else(|| Some(candidate.endpoint.pane_id.clone())),
            tmux_endpoint: Some(candidate.endpoint.clone()),
            capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
            self_check: "tmux socket, session, pane and pane pid verified".into(),
        });
    }
    Err("TRANSPORT_NONE: no reachable App Server, tmux or dsh candidate was supplied".into())
}

/// Admits a dsh candidate by challenging the gateway control socket once.
///
/// Every field must agree in the *same* response: the nonce proves the reply
/// belongs to this challenge, and runtime/agent/session/cwd are compared
/// field-by-field. Any mismatch is a rejection, never a degraded admission.
fn admit_dsh_candidate(
    candidate: &crate::proto::DshCandidate,
    candidate_root: &Path,
) -> Result<SelectedTransport, String> {
    if !candidate.cwd.starts_with('/') {
        return Err("DSH_ENDPOINT_REJECTED: dsh candidate cwd must be absolute".into());
    }
    let dsh_cwd = std::fs::canonicalize(&candidate.cwd)
        .map_err(|error| format!("DSH_ENDPOINT_REJECTED: dsh candidate cwd: {error}"))?;
    if dsh_cwd != candidate_root {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: dsh candidate cwd {} does not match project root {}",
            dsh_cwd.display(),
            candidate_root.display()
        ));
    }
    let facts = crate::client::adapters::dsh::facts(
        &candidate.endpoint,
        &candidate.runtime_id,
        &candidate.agent_id,
    )
    .map_err(|error| error.to_string())?;
    if facts.runtime_id != candidate.runtime_id {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: gateway reports runtime {} for candidate runtime {}",
            facts.runtime_id, candidate.runtime_id
        ));
    }
    if facts.agent_id != candidate.agent_id {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: gateway reports agent {} for candidate agent {}",
            facts.agent_id, candidate.agent_id
        ));
    }
    if facts.session_id != candidate.session_id {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: gateway reports session {} for candidate session {}",
            facts.session_id, candidate.session_id
        ));
    }
    if facts.status.trim().is_empty() {
        return Err("DSH_ENDPOINT_REJECTED: gateway reported an empty agent status".into());
    }
    let reported_cwd = std::fs::canonicalize(&facts.cwd).map_err(|error| {
        format!(
            "DSH_ENDPOINT_REJECTED: gateway reported dsh cwd {}: {error}",
            facts.cwd
        )
    })?;
    if reported_cwd != *candidate_root {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: gateway reports dsh cwd {} outside project root {}",
            reported_cwd.display(),
            candidate_root.display()
        ));
    }
    Ok(SelectedTransport {
        kind: TransportKind::Dsh,
        endpoint: Some(candidate.endpoint.clone()),
        namespace: Some(candidate.runtime_id.clone()),
        session_id: Some(candidate.session_id.clone()),
        thread_id: Some(candidate.agent_id.clone()),
        tmux_endpoint: None,
        capabilities: vec!["enqueue_wake".into(), "agent_facts".into()],
        self_check: format!(
            "gateway control socket answered a single-use nonce challenge; runtime, agent, session and cwd verified; reported status {}",
            facts.status
        ),
    })
}

type RouteKey = (String, String);

/// A host route is only a small admission record.  The reducer and mailbox
/// facts belong to the runtime selected by this record, never to the
/// resident project's journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostRouteRecord {
    pub(crate) version: u8,
    pub(crate) op: String,
    pub(crate) app_scope_id: String,
    pub(crate) project_scope: String,
    pub(crate) canonical_root: String,
    pub(crate) storage_root: String,
    pub(crate) registered_ms: i64,
}

pub(crate) const ROUTE_RESOLVE_NOT_FOUND_RECOVERY: &str = "recovery: run `collab context` from the canonical project main checkout; it resolves the canonical root, restores identity and registration, starts the daemon when no explicit DOWN marker exists, and returns the current role's operations. Do not re-register a worktree, edit routes.jsonl, copy identity tokens, start a second daemon, or use mailbox state as transport delivery";

struct RuntimeRoute {
    root: PathBuf,
    storage_root: PathBuf,
    runtime: Option<Arc<Server>>,
}

/// Owns the single host listener's route table and the independent project
/// reducers behind it.  The existing handler surface remains unchanged: a
/// request is first routed here, then dispatched to the selected `Server`.
struct ProjectRuntimeManager {
    host: Arc<Server>,
    host_root: PathBuf,
    route_journal: PathBuf,
    routes: Mutex<std::collections::BTreeMap<RouteKey, RuntimeRoute>>,
    project_locks: Mutex<std::collections::BTreeMap<PathBuf, std::fs::File>>,
    register_gate: Mutex<()>,
    runtime_init_gates: Mutex<std::collections::BTreeMap<RouteKey, Arc<Mutex<()>>>>,
    #[cfg(test)]
    fail_current_thread_route_publish: std::sync::atomic::AtomicBool,
}

fn storage_owner_path(path: &Path) -> Result<PathBuf, String> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut missing_suffix = Vec::new();
            let mut current = path.to_path_buf();
            loop {
                match std::fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        let target = std::fs::read_link(&current).map_err(|error| {
                            format!("storage owner symlink {}: {error}", current.display())
                        })?;
                        let target = if target.is_absolute() {
                            target
                        } else {
                            current
                                .parent()
                                .filter(|parent| !parent.as_os_str().is_empty())
                                .unwrap_or_else(|| Path::new("."))
                                .join(target)
                        };
                        let mut resolved = storage_owner_path(&target)?;
                        for component in missing_suffix.iter().rev() {
                            resolved.push(component);
                        }
                        return Ok(resolved);
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("storage owner path {}: {error}", current.display()));
                    }
                }
                let Some(name) = current.file_name() else {
                    return Err(format!(
                        "storage owner path has no resolvable ancestor: {}",
                        path.display()
                    ));
                };
                missing_suffix.push(name.to_os_string());
                let parent = current
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                match std::fs::canonicalize(parent) {
                    Ok(mut resolved) => {
                        for component in missing_suffix.iter().rev() {
                            resolved.push(component);
                        }
                        return Ok(resolved);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        current = parent.to_path_buf();
                    }
                    Err(error) => {
                        return Err(format!(
                            "storage owner path ancestor {}: {error}",
                            parent.display()
                        ));
                    }
                }
            }
        }
        Err(error) => Err(format!("storage owner path {}: {error}", path.display())),
    }
}

fn storage_roots_equal(left: &Path, right: &Path) -> Result<bool, String> {
    Ok(storage_owner_path(left)? == storage_owner_path(right)?)
}

fn is_resident_self_route(
    app_scope_id: &str,
    project_scope: &str,
    canonical_root: &Path,
    storage_root: &Path,
    host_root: &Path,
    host_storage_root: &Path,
    host: &Arc<Server>,
    existing_routes: &std::collections::BTreeMap<RouteKey, RuntimeRoute>,
) -> Result<bool, String> {
    if canonical_root != host_root || project_scope != host_root.to_string_lossy() {
        return Ok(false);
    }
    let storage_is_host_owned = storage_roots_equal(storage_root, host_root)?
        || storage_roots_equal(storage_root, host_storage_root)?;
    if !storage_is_host_owned {
        return Ok(false);
    }
    match existing_routes.get(&(app_scope_id.to_owned(), project_scope.to_owned())) {
        Some(route) => Ok(route
            .runtime
            .as_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, host))),
        // The durable route record is the recovery evidence when the
        // registration did not survive replay. Canonical root plus a
        // host-owned storage root cannot be an ordinary non-resident route.
        None => Ok(true),
    }
}

fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no parent directory",
        )
    })?;
    std::fs::File::open(parent)?.sync_all()
}

include!("append_host_route_record.rs");

fn validate_runtime_storage_root(
    root: &Path,
    storage_root: &Path,
    error_prefix: &str,
) -> Result<PathBuf, String> {
    if !storage_root.is_absolute() {
        return Err(format!(
            "{error_prefix}: runtime storage root must be absolute"
        ));
    }
    let root_owner = storage_owner_path(root)
        .map_err(|error| format!("{error_prefix}: resolve project storage owner: {error}"))?;
    let storage_owner = storage_owner_path(storage_root)
        .map_err(|error| format!("{error_prefix}: resolve runtime storage owner: {error}"))?;
    if storage_owner == root_owner {
        return Ok(root_owner);
    }
    let expected_parent = root_owner
        .join(".agent-collab")
        .join("server")
        .join("runtimes");
    if !storage_owner.starts_with(&expected_parent) {
        return Err(format!(
            "{error_prefix}: runtime storage root {} resolves outside project runtime storage {}",
            storage_root.display(),
            expected_parent.display()
        ));
    }
    Ok(storage_owner)
}

fn validate_route_owner_table(
    host: &Arc<Server>,
    routes: &std::collections::BTreeMap<RouteKey, RuntimeRoute>,
) -> Result<(), String> {
    let mut owners = std::collections::BTreeMap::<PathBuf, (String, bool)>::new();
    owners.insert(
        storage_owner_path(&host.root)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?,
        ("resident host".into(), true),
    );
    owners.insert(
        storage_owner_path(&host.storage_root)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?,
        ("resident host".into(), true),
    );

    for (key, route) in routes {
        let route_owner = format!("route ({}, {})", key.0, key.1);
        let is_resident_runtime = route
            .runtime
            .as_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, host));
        let route_storage_root = storage_owner_path(&route.storage_root)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?;
        if let Some((owner, is_host)) = owners.get(&route_storage_root) {
            if is_resident_runtime && *is_host {
                continue;
            }
            return Err(format!(
                "HOST_ROUTE_REPLAY_FAILED: runtime storage root {} is already owned by {}",
                route.storage_root.display(),
                owner
            ));
        }
        owners.insert(route_storage_root, (route_owner, is_resident_runtime));
    }
    Ok(())
}
