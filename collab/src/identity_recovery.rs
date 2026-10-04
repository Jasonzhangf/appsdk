/// Why identity loading may not mint a brand-new peer for this project.
#[derive(Debug)]
enum ScopeRebindOutcome {
    /// One durable identity matches a current tmux/Codex anchor.
    Adopted(Identity),
    /// No durable record left to protect: first registration for this project,
    /// or every stale record was provably dead and has been archived.
    NoCandidate,
    /// A *live* durable peer already claims this exact anchor. Adopting another
    /// record would collide with a reachable peer, so the caller must fail
    /// closed and require the explicit `--worker` override.
    Unproven(String),
}

/// Whether a persisted peer can still be reached. Only `Dead` authorizes
/// retiring the record and must never be treated as a live conflict. A probe
/// that merely failed is `Unknown`, and a cold-but-resumable record is `Cold`:
/// "cannot prove it is gone" and "not currently loaded" are both distinct from
/// "it is gone" and from "it is live". Only a *live* overlap is a hard
/// conflict; a `Cold`/`Unknown` record is not live, so it never blocks recovery
/// and is instead a normal drift/restart candidate the current pane adopts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerLiveness {
    /// Actively reachable: a live tmux pane or a loaded AppServer thread.
    Live,
    /// Provably gone: a missing pane, an errored thread, or an explicit
    /// not-found/rolled-out probe result. Only this authorizes retirement.
    Dead,
    /// Not currently live but resumable: an AppServer thread that is not
    /// loaded. It is preserved (never archived) and is not a live conflict.
    Cold,
    /// Liveness could not be established (probe error, malformed address, or
    /// a missing transport). It is not live, so it never blocks recovery; like
    /// `Cold` it is a normal drift/restart candidate the current pane adopts.
    Unknown,
}

fn persisted_peer_liveness(identity: &Identity) -> PeerLiveness {
    let Some(transport) = identity.transport.as_ref() else {
        // No transport cannot be proven dead. Fail closed: a record with no
        // re-anchor is still protected unless an endpoint probe proves it is
        // gone.
        return PeerLiveness::Unknown;
    };
    match transport.kind {
        TransportKind::Tmux => {
            let Some(endpoint) = transport.tmux_endpoint.as_ref() else {
                return PeerLiveness::Unknown;
            };
            match crate::client::adapters::tmux::probe(endpoint) {
                Ok(crate::client::adapters::tmux::PanePresence::Present) => PeerLiveness::Live,
                Ok(crate::client::adapters::tmux::PanePresence::Missing) => PeerLiveness::Dead,
                Ok(crate::client::adapters::tmux::PanePresence::Unknown) | Err(_) => {
                    PeerLiveness::Unknown
                }
            }
        }
        TransportKind::AppServer => {
            let Some(thread_id) = transport.thread_id.as_deref() else {
                return PeerLiveness::Unknown;
            };
            match crate::client::adapters::codex_app_server::read_thread_status(
                transport, thread_id,
            ) {
                Ok(raw) => classify_thread_status(&raw),
                Err(error) => classify_probe_error(&error.to_string()),
            }
        }
        TransportKind::Dsh => {
            // A dsh peer is re-anchored by challenging the gateway, not by
            // comparing a pane. Only an explicit "the gateway does not know
            // this agent" retires the record: a gateway that is down, slow or
            // answering garbage says nothing about the agent, so it stays
            // `Unknown` and never authorizes retirement.
            let (Some(endpoint), Some(runtime_id), Some(agent_id)) = (
                transport.endpoint.as_deref(),
                transport.namespace.as_deref(),
                transport.thread_id.as_deref(),
            ) else {
                return PeerLiveness::Unknown;
            };
            match crate::client::adapters::dsh::probe(endpoint, runtime_id, agent_id) {
                crate::client::adapters::dsh::PeerPresence::Live => PeerLiveness::Live,
                crate::client::adapters::dsh::PeerPresence::Absent => PeerLiveness::Dead,
                crate::client::adapters::dsh::PeerPresence::Unknown => PeerLiveness::Unknown,
            }
        }
    }
}

/// Only an explicitly dead signal retires the record. A `notLoaded` thread is
/// cold, not gone: the AppServer contract can resume it through `turn/start`,
/// so it is classified as non-live and stays recoverable instead of being
/// archived.
fn classify_thread_status(raw: &serde_json::Value) -> PeerLiveness {
    match raw
        .pointer("/thread/status/type")
        .and_then(serde_json::Value::as_str)
    {
        Some("systemError") => PeerLiveness::Dead,
        // A successful read that reports `notLoaded` is a definitive "not
        // currently live, but resumable" answer, not an unproven one.
        Some("notLoaded") => PeerLiveness::Cold,
        Some(_) => PeerLiveness::Live,
        None => PeerLiveness::Unknown,
    }
}

/// Only an explicitly dead signal retires the record. A malformed App Server
/// response (a missing field, a decode failure) is unproven, not dead: the
/// endpoint answered, so the thread may still be live and must not authorize a
/// credential retirement. `notLoaded` is reported by `classify_thread_status`,
/// not here.
fn classify_probe_error(detail: &str) -> PeerLiveness {
    let lowered = detail.to_ascii_lowercase();
    if lowered.contains("no rollout")
        || lowered.contains("thread not found")
        || lowered.contains("tmux_pane_missing")
    {
        PeerLiveness::Dead
    } else {
        PeerLiveness::Unknown
    }
}

/// Durable registration recency for one persisted identity. The identity file
/// is rewritten atomically on every registration, so its last write time is a
/// globally ordered "when was this record last refreshed" signal that stays
/// valid across different worker/binding ids. A per-binding
/// `endpoint_generation` restarts at 1 for a new binding and is therefore not
/// a global order.
fn identity_recency_at(host_paths: &HostPaths, worker_id: &str) -> std::time::SystemTime {
    let path = host_paths
        .state_root()
        .join("identities")
        .join(worker_id)
        .join("identity.json");
    std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

/// Deterministic preference between two records that both match the current
/// anchor and are not live. Prefer the record this pane derives its own id
/// from (`codex-<pane>`), then the most recently registered record, then the
/// lowest worker id so the outcome is stable even on an exact timestamp tie.
fn non_live_candidate_order(
    host_paths: &HostPaths,
    candidate: Option<&crate::proto::TmuxCandidate>,
    left: &Identity,
    right: &Identity,
) -> std::cmp::Ordering {
    let derived = candidate.map(|candidate| format!("codex-{}", candidate.endpoint.pane_id));
    let left_own = derived.as_deref() == Some(left.worker_id.as_str());
    let right_own = derived.as_deref() == Some(right.worker_id.as_str());
    right_own
        .cmp(&left_own)
        .then_with(|| {
            identity_recency_at(host_paths, &right.worker_id)
                .cmp(&identity_recency_at(host_paths, &left.worker_id))
        })
        .then_with(|| left.worker_id.cmp(&right.worker_id))
}

/// Resolve one anchor that several persisted records claim.
///
/// Project scope is applied first: a record registered under another project
/// can never be the current project's peer for the same anchor, so a foreign
/// duplicate can neither shadow nor be retired in place of a current-scope
/// match. Every member here matches this exact anchor, so only a *live* member
/// is a hard conflict: two reachable peers cannot share one anchor, so the
/// explicit `--worker` override must decide. Dead, cold, and unproven members
/// are all non-live, so the deterministic non-live order adopts this peer's own
/// drifted registration instead of blocking recovery.
fn choose_anchor_peer(
    host_paths: &HostPaths,
    candidate: Option<&crate::proto::TmuxCandidate>,
    expected_scope: &crate::scope::ProjectScopeId,
    anchor: &str,
    mut candidates: Vec<Identity>,
    liveness: impl Fn(&Identity) -> PeerLiveness,
) -> anyhow::Result<Identity> {
    // A reachable peer that claims this anchor is a hard conflict regardless of
    // which project scope it registered under. Scope preference must never hide
    // a live owner (including a foreign one) behind a non-live current-scope
    // duplicate.
    if candidates
        .iter()
        .any(|identity| matches!(liveness(identity), PeerLiveness::Live))
    {
        anyhow::bail!(
            "IDENTITY_RESTORE_AMBIGUOUS: {anchor} matches multiple live peers; pass --worker <worker_id> to explicitly select one"
        );
    }
    let in_scope = candidates
        .iter()
        .filter(|identity| identity.project_scope.as_ref() == Some(expected_scope))
        .cloned()
        .collect::<Vec<_>>();
    if !in_scope.is_empty() {
        candidates = in_scope;
        // A provably dead record is only chosen when every same-scope record
        // that claims this anchor is provably dead. If any current-scope record
        // is still cold/unproven it is the surviving recovery candidate, so a
        // newer Dead record must never be revived by durable recency.
        let (dead, surviving): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .partition(|identity| matches!(liveness(identity), PeerLiveness::Dead));
        candidates = if surviving.is_empty() { dead } else { surviving };
    }
    candidates.sort_by(|left, right| non_live_candidate_order(host_paths, candidate, left, right));
    Ok(candidates.remove(0))
}

/// Whether a persisted peer's durable anchor overlaps the current pane or the
/// Codex session/thread being recovered. Only a peer that is *live* AND
/// overlaps this anchor can block recovery; a live peer on an unrelated pane
/// or thread must not gate a fresh pane or an anchor-drift recovery.
///
/// A tmux peer is addressed by its pane, but an App Server peer is addressed by
/// its Codex session/thread and may keep a tmux recovery anchor. The pane is
/// therefore only an anchor for a tmux transport, or when the current process
/// carries no Codex IDs and the pane is its only anchor; otherwise a shared
/// pane would hide a distinct live App Server thread.
///
/// The candidate's own anchors are the only reliable anchors here. Do not fall
/// back to ambient `CODEX_*` values: a candidate is an existing peer or a
/// recovered address that must be compared on its own fields.
fn identity_anchor_conflicts_with_candidate(
    identity: &Identity,
    candidate: Option<&crate::proto::TmuxCandidate>,
) -> bool {
    let current_session = candidate
        .and_then(|candidate| candidate.endpoint.codex_session_id.clone());
    let current_thread = candidate
        .and_then(|candidate| candidate.endpoint.codex_thread_id.clone());
    let runtime = identity.runtime.as_ref();
    let transport = identity.transport.as_ref();
    let persisted_session = transport
        .and_then(|transport| transport.session_id.as_deref())
        .or_else(|| {
            runtime
                .and_then(|runtime| runtime.session_id.as_ref())
                .map(|session| session.as_str())
        });
    let persisted_thread = transport
        .and_then(|transport| transport.thread_id.as_deref())
        .or_else(|| {
            runtime
                .and_then(|runtime| runtime.native_thread_id.as_ref())
                .map(|thread| thread.as_str())
        });

    let pane_is_anchor = transport.is_some_and(|transport| transport.kind == TransportKind::Tmux)
        || (current_session.is_none() && current_thread.is_none());
    if pane_is_anchor {
        if let Some(candidate) = candidate {
            if let Some(endpoint) = transport.and_then(|transport| transport.tmux_endpoint.as_ref())
            {
                if crate::client::adapters::tmux::same_pane_route(endpoint, &candidate.endpoint) {
                    return true;
                }
            }
        }
    }

    if let (Some(left), Some(right)) = (persisted_session, current_session.as_deref()) {
        if left == right {
            return true;
        }
    }
    if let (Some(left), Some(right)) = (persisted_thread, current_thread.as_deref()) {
        if left == right {
            return true;
        }
    }
    false
}

/// Move provably dead peers out of the live identity set so a new pane can
/// register. The bytes are archived, never deleted, and only peers whose
/// endpoint is *proven* gone are retired.
fn archive_dead_peers(host_paths: &HostPaths, dead: &[Identity]) -> anyhow::Result<()> {
    if dead.is_empty() {
        return Ok(());
    }
    let archive_root = host_paths
        .state_root()
        .join("archives")
        .join(format!("identities-retired-{}", now_ms()));
    std::fs::create_dir_all(&archive_root)?;
    for identity in dead {
        validate_id(&identity.worker_id)?;
        let source = host_paths
            .state_root()
            .join("identities")
            .join(&identity.worker_id);
        let destination = archive_root.join(&identity.worker_id);
        std::fs::rename(&source, &destination).with_context(|| {
            format!(
                "IDENTITY_RETIRE_FAILED: cannot archive stale peer {} at {}",
                identity.worker_id,
                source.display()
            )
        })?;
    }
    Ok(())
}

/// Resolve one persisted same-scope identity to the current process.
///
/// Recovery is single-source (persisted identities + current project scope +
/// transport liveness) and single-sink. Normal session/thread/pane drift
/// adopts the unique durable candidate even when current anchors no longer
/// match, and provably dead records are archived. Only a *live* record that
/// overlaps the current anchor is a hard conflict: two reachable peers cannot
/// share one anchor. Cold/unproven records are not live, so they never block:
/// whether overlapping or not, they are normal drift/restart candidates the
/// current pane adopts deterministically. The explicit user override
/// (selected_worker) is resolved first and directly by path, before any anchor
/// or liveness work, so the named durable identity is always recoverable on
/// request without probing the very records it exists to bypass.
fn identity_for_scope_rebind_at(
    host_paths: &HostPaths,
    scope: &Scope,
    selected_worker: Option<&str>,
) -> anyhow::Result<ScopeRebindOutcome> {
    // The explicit override names the durable identity to recover, so it is
    // resolved first, directly by path, before any anchor or liveness work: it
    // exists precisely to bypass an ambiguous anchor or a stale/silent peer, so
    // probing the very records it must bypass could delay the recovery by the
    // probe timeout or fail it outright. A name with no record is
    // `NoCandidate`; the caller decides whether that may mint.
    if let Some(selected) = selected_worker {
        return Ok(
            match read_identity(&identity_path_at(host_paths, selected)?)? {
                Some(identity) => ScopeRebindOutcome::Adopted(identity),
                None => ScopeRebindOutcome::NoCandidate,
            },
        );
    }
    let project_scope = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
        .project_scope_id;
    // Adopting a persisted peer only re-anchors it, so a caller must hold a
    // current anchor to ask for one. Without any pane, Codex session or thread
    // address the caller stays unauthenticated and only an explicit `--worker`
    // may name a durable identity.
    if std::env::var_os("TMUX_PANE").is_none()
        && std::env::var_os("CODEX_SESSION_ID").is_none()
        && std::env::var_os("CODEX_THREAD_ID").is_none()
    {
        return Ok(ScopeRebindOutcome::Unproven(
            "no current anchor; pass --worker to explicitly recover a durable identity".into(),
        ));
    }
    let candidate = if std::env::var_os("TMUX_PANE").is_some() {
        Some(crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?)
    } else {
        None
    };
    if let Some(identity) =
        identity_by_current_anchors_same_scope_at(host_paths, scope, candidate.as_ref())?
    {
        return Ok(ScopeRebindOutcome::Adopted(identity));
    }
    let identities_root = host_paths.state_root().join("identities");
    if !identities_root.is_dir() {
        return Ok(ScopeRebindOutcome::NoCandidate);
    }
    // Every same-scope record lands in exactly one bucket: provably dead,
    // live conflict (a reachable peer that claims this exact anchor), or a
    // non-live recovery candidate (cold/unproven anywhere, or a live peer on an
    // unrelated anchor that is neither ours nor blocking).
    let mut dead = Vec::new();
    let mut live_conflict = Vec::new();
    let mut recoverable = Vec::new();
    for entry in std::fs::read_dir(identities_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(identity) = read_identity(&entry.path().join("identity.json"))? else {
            continue;
        };
        if identity.project_scope.as_ref() != Some(&project_scope) {
            continue;
        }
        let overlaps = identity_anchor_conflicts_with_candidate(&identity, candidate.as_ref());
        match persisted_peer_liveness(&identity) {
            PeerLiveness::Dead => dead.push(identity),
            // Only a *live* record that claims the current pane/session/thread
            // is a hard conflict: two reachable peers cannot share one anchor,
            // so recovery must fail closed and let the explicit `--worker`
            // override resolve it.
            PeerLiveness::Live if overlaps => live_conflict.push(identity),
            // A live record on an unrelated anchor is a different, healthy
            // peer: it neither blocks nor is adopted.
            PeerLiveness::Live => {}
            // Cold/Unknown records are not live, so they never conflict. They
            // are the normal drift/restart candidates: the current pane adopts
            // the best one and registration refreshes its stale anchor.
            PeerLiveness::Cold | PeerLiveness::Unknown => recoverable.push(identity),
        }
    }
    // Provably dead same-scope records never block recovery.
    archive_dead_peers(host_paths, &dead)?;

    // A live peer that claims the current anchor is a real conflict, not drift:
    // it is reachable and must not be silently superseded. Fail closed so the
    // explicit `--worker` override decides.
    if !live_conflict.is_empty() {
        let conflict_ids = live_conflict
            .iter()
            .map(|identity| identity.worker_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(ScopeRebindOutcome::Unproven(format!(
            "live peers overlap the current anchor ({conflict_ids}); pass --worker to explicitly override before recovery"
        )));
    }

    // No live peer claims this anchor, so this is normal drift or a restart.
    // Adopt the deterministic best non-live record (a record on the current
    // anchor first, then the pane-derived id, then durable recency) and let
    // registration refresh its stale anchor. With no candidate this is a first
    // registration for the project.
    if recoverable.is_empty() {
        return Ok(ScopeRebindOutcome::NoCandidate);
    }
    recoverable.sort_by(|left, right| {
        let left_overlap = identity_anchor_conflicts_with_candidate(left, candidate.as_ref());
        let right_overlap = identity_anchor_conflicts_with_candidate(right, candidate.as_ref());
        right_overlap
            .cmp(&left_overlap)
            .then_with(|| non_live_candidate_order(host_paths, candidate.as_ref(), left, right))
    });
    Ok(ScopeRebindOutcome::Adopted(recoverable.remove(0)))
}
