/// Explicit runtime anchors observed for one caller.
///
/// The daemon resolver is built only from typed `IdentityFacts`. The legacy
/// `from_env` adapter exists for the read-only board/scope lookups that still
/// derive the caller's own anchors from the process environment.
#[derive(Debug, Clone, Default)]
struct AnchorObservation {
    session_id: Option<String>,
    thread_id: Option<String>,
    tmux: Option<crate::proto::TmuxCandidate>,
    /// The dsh adapter's primary anchor. Read from `DSH_SESSION_ID`, which the
    /// DSH runtime exports for the current agent session.
    dsh_session_id: Option<String>,
}

impl AnchorObservation {
    fn from_facts(facts: &crate::proto::IdentityFacts) -> Self {
        Self {
            session_id: non_empty(facts.session_id.as_deref()),
            thread_id: non_empty(facts.thread_id.as_deref()),
            tmux: facts.tmux.clone(),
            dsh_session_id: non_empty(facts.dsh_session_id.as_deref()),
        }
    }

    /// Legacy adapter for the read-only CLI callers (`load_existing_at`). The
    /// daemon path never uses this: it consumes only `IdentityFacts`.
    fn from_env() -> anyhow::Result<Self> {
        let tmux = if std::env::var_os("TMUX_PANE").is_some() {
            Some(crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?)
        } else {
            None
        };
        let session_env = std::env::var("CODEX_SESSION_ID").ok();
        let thread_env = std::env::var("CODEX_THREAD_ID").ok();
        Ok(Self {
            session_id: non_empty(session_env.as_deref()),
            thread_id: non_empty(thread_env.as_deref()),
            tmux,
            dsh_session_id: non_empty(std::env::var("DSH_SESSION_ID").ok().as_deref()),
        })
    }

    /// Every Codex session anchor this observation carries: the explicit fact
    /// plus any session id the auto-observed tmux candidate recorded.
    fn session_anchors(&self) -> Vec<String> {
        let mut anchors = Vec::new();
        if let Some(session) = self.session_id.clone() {
            anchors.push(session);
        }
        if let Some(session) = self
            .tmux
            .as_ref()
            .and_then(|tmux| tmux.endpoint.codex_session_id.clone())
        {
            if !anchors.contains(&session) {
                anchors.push(session);
            }
        }
        anchors
    }

    /// Every Codex thread anchor this observation carries.
    fn thread_anchors(&self) -> Vec<String> {
        let mut anchors = Vec::new();
        if let Some(thread) = self.thread_id.clone() {
            anchors.push(thread);
        }
        if let Some(thread) = self
            .tmux
            .as_ref()
            .and_then(|tmux| tmux.endpoint.codex_thread_id.clone())
        {
            if !anchors.contains(&thread) {
                anchors.push(thread);
            }
        }
        anchors
    }

    fn has_anchor(&self) -> bool {
        self.session_id.is_some()
            || self.thread_id.is_some()
            || self.tmux.is_some()
            || self.dsh_session_id.is_some()
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Resolve the durable identity for one host-local bootstrap from typed facts.
pub(crate) fn existing_for_daemon_at(
    host_paths: &HostPaths,
    scope: &Scope,
    facts: &crate::proto::IdentityFacts,
) -> anyhow::Result<Option<Identity>> {
    identity_by_current_anchors_same_scope_at(
        host_paths,
        scope,
        &AnchorObservation::from_facts(facts),
    )
}

/// Resolve the durable identity for one host-local bootstrap from typed facts.
///
/// The result is either an existing persisted identity (with its exact stored
/// token) or an in-memory draft. This function never writes, archives or calls
/// back into the daemon. The caller runs the existing Register transaction and
/// only then persists the receipt through `persist_registration_at`.
pub(crate) fn resolve_for_daemon_at(
    host_paths: &HostPaths,
    scope: &Scope,
    facts: &crate::proto::IdentityFacts,
) -> anyhow::Result<Identity> {
    resolve_for_daemon_with_route_at(host_paths, scope, facts, None)
}

/// Route-aware variant for archived same-pane recovery. The daemon supplies
/// the current committed `RouteResolution` for the pane so the archived
/// credential is validated against live host route/binding evidence. With no
/// route the recovery reads only durable archive files; it never issues a
/// daemon-to-self RPC.
pub(crate) fn resolve_for_daemon_with_route_at(
    host_paths: &HostPaths,
    scope: &Scope,
    facts: &crate::proto::IdentityFacts,
    route: Option<&crate::proto::RouteResolution>,
) -> anyhow::Result<Identity> {
    let observed = AnchorObservation::from_facts(facts);
    if !observed.has_anchor() {
        anyhow::bail!(
            "COLLAB_IDENTITY_ANCHOR_MISSING: identity requires a Codex session/thread, a tmux pane, or a native App Server endpoint"
        );
    }
    // Recover only from anchors the caller actually supplied. An unrelated
    // same-project cold/unknown peer is never adopted by recency.
    if let Some(identity) = identity_by_current_anchors_same_scope_at(host_paths, scope, &observed)?
    {
        return Ok(identity);
    }
    if let Some(candidate) = observed.tmux.as_ref() {
        if let Some(identity) = recover_archived_pane_at(host_paths, scope, candidate, route)? {
            return Ok(identity);
        }
    }
    let worker_id = draft_worker_id_at(&observed)?;
    // Preserve an existing draft's exact token; never remint a stored credential.
    if let Some(existing) = read_identity(&identity_path_at(host_paths, &worker_id)?)? {
        let expected_scope = scope
            .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
            .project_scope_id;
        if existing.project_scope.as_ref() != Some(&expected_scope) {
            anyhow::bail!("IDENTITY_RESTORE_CROSS_PROJECT: the derived identity name belongs to another project");
        }
        if existing.runtime.is_some() {
            anyhow::bail!("IDENTITY_RESTORE_CONFLICT: the derived identity name has a registered runtime with different observed anchors");
        }
        return Ok(existing);
    }
    draft_identity_with_id(scope, &worker_id)
}

/// Deterministic id for a genuine no-match anchor. Preserves the canonical
/// `codex-<pane>` and `codex-thread-<hex>` naming so a fresh verified anchor
/// keeps its stable, filesystem-safe identity.
fn draft_worker_id_at(observed: &AnchorObservation) -> anyhow::Result<String> {
    if let Some(tmux) = observed.tmux.as_ref() {
        let worker_id = format!("codex-{}", tmux.endpoint.pane_id);
        validate_id(&worker_id)?;
        return Ok(worker_id);
    }
    if let Some(dsh_session) = observed.dsh_session_id.as_deref() {
        let worker_id = format!("dsh-thread-{}", hex_encode(dsh_session));
        validate_id(&worker_id)?;
        return Ok(worker_id);
    }
    let thread_id = observed.thread_id.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "COLLAB_IDENTITY_ANCHOR_MISSING: a native App Server identity requires a thread id"
        )
    })?;
    let worker_id = format!("codex-thread-{}", hex_encode(thread_id));
    validate_id(&worker_id)?;
    Ok(worker_id)
}

/// Hex-encode an anchor value so it is safe as a directory name without
/// dropping characters the anchor may legitimately contain.
fn hex_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

/// In-memory draft for a new verified anchor. No credential is written before
/// Register succeeds; the daemon persists the committed receipt later.
fn draft_identity_with_id(scope: &Scope, worker_id: &str) -> anyhow::Result<Identity> {
    validate_id(worker_id)?;
    let project_scope = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
        .project_scope_id;
    Ok(Identity {
        worker_id: worker_id.to_owned(),
        token: hex(16),
        project_scope: Some(project_scope),
        runtime: None,
        transport: None,
    })
}
