/// A historical workspace string that cannot prove a local-path declaration.
/// Dedicated declaration paths may contain colons; lifecycle consumers use
/// this predicate only to fail closed on old ambiguous references.
fn workspace_reference_is_ambiguous(value: &str) -> bool {
    let trimmed = value.trim_start();
    let first_component = trimmed
        .split(['/', '\\'])
        .next()
        .unwrap_or_default();
    !trimmed.starts_with("./")
        && !trimmed.starts_with("../")
        && !trimmed.starts_with('/')
        && !trimmed.starts_with("\\\\")
        && first_component.contains(':')
}

/// The resident daemon owns one current project-state reducer. Keep that
/// ownership explicit at the host boundary instead of treating the process
/// root as an implicit fallback for every valid project context.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostRouteOwner {
    ResidentProject { root: PathBuf },
    RegisteredNotReady { root: PathBuf },
}

#[derive(Debug, Clone, Default)]
struct HostRouteRegistry {
    /// The route identity is the pair, rather than the project alone.  A
    /// project may be registered by more than one appserver, but this v1
    /// daemon has one project reducer and therefore admits only its resident
    /// app route.
    routes: std::collections::BTreeMap<(String, String), HostRouteOwner>,
}

impl HostRouteRegistry {
    fn for_server(server: &Server) -> Result<Self, String> {
        let resident_scope = GlobalState::canonical_project_scope(&server.root)
            .map_err(|error| format!("PROJECT_SCOPE_UNKNOWN: {error}"))?;
        let resident_root = PathBuf::from(resident_scope.as_str());
        // GlobalState is the only durable registration index available to the
        // v1 resident process.  A registered project without a resident
        // reducer remains visible as a route, but cannot be sent to this
        // project's State until a separate runtime owner is available.
        let st = server.state.lock().unwrap();
        let resident_app_scope = st
            .global
            .lookup_project(&resident_scope)
            .and_then(|project| {
                let mut bound_apps = project
                    .runtime_bindings
                    .values()
                    .map(|binding| binding.app_scope_id.as_str().to_owned())
                    .collect::<std::collections::BTreeSet<_>>();
                match bound_apps.len() {
                    1 => bound_apps
                        .pop_first()
                        .and_then(|app_scope| project.registrations.get(&app_scope))
                        .map(|registration| registration.app_scope_id.clone()),
                    0 if project.registrations.len() == 1 => project
                        .registrations
                        .values()
                        .next()
                        .map(|registration| registration.app_scope_id.clone()),
                    _ => None,
                }
            });
        let mut routes = std::collections::BTreeMap::new();
        for project in st.global.projects.values() {
            for registration in project.registrations.values() {
                let project_scope = registration.project_scope.as_str().to_owned();
                let app_scope = registration.app_scope_id.as_str().to_owned();
                let owner = if registration.project_scope == resident_scope
                    && resident_app_scope.as_ref() == Some(&registration.app_scope_id)
                {
                    HostRouteOwner::ResidentProject {
                        root: resident_root.clone(),
                    }
                } else {
                    HostRouteOwner::RegisteredNotReady {
                        root: PathBuf::from(project_scope.clone()),
                    }
                };
                routes.insert((app_scope, project_scope), owner);
            }
        }
        Ok(Self { routes })
    }

    fn lookup(&self, context: &ProjectContext) -> Option<&HostRouteOwner> {
        self.routes.get(&(
            context.app_scope_id.as_str().to_owned(),
            context.project_scope.as_str().to_owned(),
        ))
    }
}

const MAX_POLL_MS: u64 = 3_600_000;
const TASK_STATUSES: [&str; 14] = [
    "pending",
    "invited",
    "assigned",
    "working",
    "blocked",
    "waiting",
    "verifying",
    "reviewed",
    "delivered",
    "accepted",
    "rework",
    "merged",
    "closed",
    "cancelled",
];
/// Lock used by releases before the host-scoped state directory existed.
/// A new daemon must fence this writer before loading current state;
/// otherwise an old binary could mutate the same files under the new socket.
const LEGACY_HOST_DAEMON_LOCK_PATH: &str = "/tmp/collab-host.lock";
const DAEMON_LIVE_CLOSURE_MODE: &str = "daemon-live-closure";
const RESTART_REPLAY_PENDING_MODE: &str = "restart-replay-pending";

type AppServerCandidateCheck =
    dyn Fn(&crate::proto::AppServerCandidate) -> Result<SelectedTransport, String> + Send + Sync;
type TmuxNotificationSink = dyn Fn(
        &SelectedTransport,
        Option<&str>,
        &str,
        &str,
        bool,
        &str,
    ) -> Result<serde_json::Value, String>
    + Send
    + Sync;
type AppServerThreadStatus =
    dyn Fn(&SelectedTransport, &str) -> Result<serde_json::Value, String> + Send + Sync;
type AppServerThreadArchive =
    dyn Fn(&SelectedTransport, &str) -> Result<serde_json::Value, String> + Send + Sync;

fn default_appserver_candidate_check() -> Arc<AppServerCandidateCheck> {
    Arc::new(|candidate| {
        crate::client::adapters::verify_candidate(candidate).map_err(|error| error.to_string())
    })
}

fn default_tmux_notification_sink() -> Arc<TmuxNotificationSink> {
    Arc::new(
        |transport, source_thread_id, body, message_id, explicit, mode| {
            if transport.kind != TransportKind::Tmux {
                return Err("TRANSPORT_UNSUPPORTED: Collab notifications require tmux".into());
            }
            let _ = (source_thread_id, explicit);
            match mode {
                "immediate" | "explicit-notification" => {}
                "queued" => {
                    return Err(
                        "TMUX_DELIVERY_MODE_UNSUPPORTED: queued delivery must remain durable until a wake-capable mode is selected"
                            .into(),
                    )
                }
                "daemon-live-closure" | "restart-replay-pending" => {}
                other => {
                    return Err(format!(
                        "TMUX_DELIVERY_MODE_UNSUPPORTED: unknown delivery mode {other}"
                    ))
                }
            }
            let endpoint = transport.tmux_endpoint.as_ref().ok_or_else(|| {
                "TMUX_ENDPOINT_MISSING: selected transport has no endpoint".to_owned()
            })?;
            crate::client::adapters::tmux::notify(endpoint, message_id, body)
        },
    )
}

/// The App Server client id that a collab notification announces itself with.
///
/// The mailbox id is the transport-neutral identity of a notification. The App
/// Server presents it inside a client-id namespace it shares with the user's
/// own messages, so the presentation is derived here and named separately
/// instead of being smuggled through the transport-neutral parameter.
pub(crate) fn appserver_client_message_id(message_id: &str) -> String {
    format!("collab-notification-{message_id}")
}

pub(crate) fn default_appserver_notification_sink() -> Arc<TmuxNotificationSink> {
    let tmux = default_tmux_notification_sink();
    Arc::new(
        move |transport, source_thread_id, body, message_id, explicit, mode| {
            // Exhaustive on purpose. This used to be `if kind != AppServer`,
            // which sent any new kind into the tmux sink and failed with
            // "Collab notifications require tmux".
            match transport.kind {
                TransportKind::DesktopBridge => crate::client::adapters::desktop_bridge::send(transport, body),
                TransportKind::AppServer => {
                    // `message_id` is the mailbox id. An App Server client id
                    // shares its namespace with the user's own messages, so a
                    // collab notification announces itself as one. The
                    // live-closure reader strips this prefix again to recover
                    // the mailbox id.
                    let client_message_id = appserver_client_message_id(message_id);
                    if mode == "queued" {
                        crate::client::adapters::codex_app_server::queued_notify(
                            transport,
                            source_thread_id,
                            body,
                            &client_message_id,
                        )
                        .map_err(|error| error.to_string())
                    } else {
                        crate::client::adapters::codex_app_server::immediate_notify(
                            transport,
                            source_thread_id,
                            body,
                            &client_message_id,
                        )
                        .map_err(|error| error.to_string())
                    }
                }
                TransportKind::Tmux => tmux(
                    transport,
                    source_thread_id,
                    body,
                    message_id,
                    explicit,
                    mode,
                ),
                TransportKind::Dsh => {
                    let endpoint = transport.endpoint.as_deref().ok_or_else(|| {
                        "DSH_ENDPOINT_REJECTED: selected dsh transport has no control socket"
                            .to_owned()
                    })?;
                    let runtime_id = transport.namespace.as_deref().ok_or_else(|| {
                        "DSH_ENDPOINT_REJECTED: selected dsh transport has no gateway runtime id"
                            .to_owned()
                    })?;
                    let agent_id = transport.thread_id.as_deref().ok_or_else(|| {
                        "DSH_ENDPOINT_REJECTED: selected dsh transport has no agent id".to_owned()
                    })?;
                    let (gateway_mode, internal_marker) = dsh_wake_mode(mode, message_id)?;
                    let mut receipt = crate::client::adapters::dsh::notify(
                        endpoint,
                        runtime_id,
                        agent_id,
                        gateway_mode,
                        source_thread_id.unwrap_or("collab"),
                        body,
                        message_id,
                    )
                    .map_err(|error| error.to_string())?;
                    if let Some(object) = receipt.as_object_mut() {
                        object.insert(
                            "sink_mode".into(),
                            serde_json::Value::String(mode.to_owned()),
                        );
                        if internal_marker {
                            object.insert(
                                "unexpected_internal_mode".into(),
                                serde_json::Value::Bool(true),
                            );
                        }
                    }
                    Ok(receipt)
                }
                TransportKind::Rex => {
                    // The notification sink is synchronous while the Rex
                    // primitive owns async process I/O. daemon callers enter
                    // this sink from the Tokio blocking pool, so block on the
                    // current runtime handle instead of starting a second one.
                    let handle = tokio::runtime::Handle::try_current().map_err(|error| {
                        format!("REX_UNKNOWN: Tokio runtime unavailable for Rex sink: {error}")
                    })?;
                    let receipt = handle
                        .block_on(crate::client::adapters::rex::notify(
                            transport,
                            body,
                            Some(mode),
                        ))
                        .map_err(|error| error.to_string())?;
                    serde_json::from_str::<serde_json::Value>(&receipt)
                        .map_err(|error| error.to_string())
                }
            }
        },
    )
}

/// Maps collab's delivery `mode` onto the gateway's `enqueue` mode.
///
/// The domain is exhaustive. The sink used to be a two-way if/else, so any
/// value other than `queued` silently took the immediate path; a new value must
/// be a deliberate decision here, not a fallthrough.
///
/// Returns the gateway mode and whether the sink mode was one of the internal
/// bookkeeping markers that should never reach a dsh wake.
fn dsh_wake_mode(mode: &str, message_id: &str) -> Result<(&'static str, bool), String> {
    match mode {
        // Start a turn.
        "immediate" => Ok(("followup", false)),
        // Cross-project explicit notification. The App Server sink sends
        // everything except `queued` through `immediate_notify`, so `followup`
        // is the behaviour-preserving mapping.
        "explicit-notification" => Ok(("followup", false)),
        // Queue without starting a turn.
        "queued" => Ok(("inject", false)),
        // Internal bookkeeping markers. Not produced for dsh; if one arrives it
        // is treated as an immediate wake rather than dropped, and the anomaly
        // is recorded in the durable receipt.
        "daemon-live-closure" | "restart-replay-pending" => Ok(("followup", true)),
        other => Err(format!(
            "DSH_ENDPOINT_REJECTED: wake {message_id} carries unknown delivery mode {other}"
        )),
    }
}

fn notification_sink(server: &Server) -> &Arc<TmuxNotificationSink> {
    &server.appserver_notification_sink
}

fn default_appserver_thread_status() -> Arc<AppServerThreadStatus> {
    Arc::new(|transport, thread_id| match transport.kind {
        TransportKind::DesktopBridge => crate::client::adapters::desktop_bridge::read_thread_status(transport, thread_id),
        // A tmux transport has no App Server endpoint to query, so this oracle
        // refuses it. The pane is an address, never a liveness credential: a
        // caller must not read presence out of a pane probe.
        TransportKind::Tmux => Err(
            "TMUX_ENDPOINT_NOT_QUERYABLE: a tmux transport has no App Server endpoint".to_owned(),
        ),
        TransportKind::AppServer => {
            crate::client::adapters::codex_app_server::read_thread_status(transport, thread_id)
                .map_err(|error| error.to_string())
        }
        TransportKind::Rex => Err(
            "REX_ENDPOINT_NOT_QUERYABLE: a rex transport has no App Server thread status".to_owned(),
        ),
        TransportKind::Dsh => {
            let endpoint = transport.endpoint.as_deref().ok_or_else(|| {
                "DSH_ENDPOINT_REJECTED: selected dsh transport has no control socket".to_owned()
            })?;
            let runtime_id = transport.namespace.as_deref().ok_or_else(|| {
                "DSH_ENDPOINT_REJECTED: selected dsh transport has no gateway runtime id".to_owned()
            })?;
            // The dsh agent id is the address; asking about a different agent
            // than the selected endpoint is a caller error, not a probe.
            if transport.thread_id.as_deref() != Some(thread_id) {
                return Err(
                    "DSH_ENDPOINT_REJECTED: requested agent does not match selected endpoint"
                        .to_owned(),
                );
            }
            let facts = crate::client::adapters::dsh::facts(endpoint, runtime_id, thread_id)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({
                "transport": "dsh",
                "runtimeId": facts.runtime_id,
                "agentId": facts.agent_id,
                "sessionId": facts.session_id,
                "cwd": facts.cwd,
                "status": facts.status,
            }))
        }
    })
}

pub(crate) fn default_appserver_thread_archive() -> Arc<AppServerThreadArchive> {
    Arc::new(|transport, _thread_id| match transport.kind {
        TransportKind::DesktopBridge | TransportKind::Tmux | TransportKind::AppServer => {
            Err("TRANSPORT_UNSUPPORTED: tmux has no Codex thread archive operation".into())
        }
        // A dsh agent's lifecycle belongs to the gateway: collab never archives
        // or retires it.
        TransportKind::Dsh => Err(
            "TRANSPORT_UNSUPPORTED: a dsh agent's lifecycle is owned by the gateway, not by collab"
                .into(),
        ),
        TransportKind::Rex => Err(
            "TRANSPORT_UNSUPPORTED: a rex agent's lifecycle is owned by the Rex server, not by collab"
                .into(),
        ),
    })
}

#[cfg(test)]
static STARTUP_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn startup_test_lock() -> std::sync::MutexGuard<'static, ()> {
    STARTUP_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn sanitize_identifier(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Encode an app scope into filesystem components without lossy sanitizing.
/// AppServerId deliberately accepts any control-free UTF-8 string, so replacing
/// path punctuation with `_` is not injective (`app/a` and `app:a` would
/// collide). Hex encodes the original bytes and fixed-size chunks keep every
/// component below common filesystem name limits even at the 256-byte ID cap.
fn app_scope_storage_path(root: &Path, app_scope: &str) -> PathBuf {
    crate::scope::encoded_storage_path(
        &root.join(".agent-collab").join("server").join("runtimes"),
        app_scope,
    )
}


#[cfg(test)]
const DIRECT_MESSAGE_WAKE_COOLDOWN_MS: i64 = 60_000;

#[derive(Debug)]
enum NotificationDeliveryError {
    State(notification_contract::MutationError),
}

#[derive(Debug)]
enum NotificationAttempt {
    Accepted,
    Consumed,
    Deferred,
    NotAttempted(String),
    Rejected(String),
}

impl NotificationAttempt {
    fn accepted(&self) -> bool {
        matches!(self, Self::Accepted)
    }
}

impl std::fmt::Display for NotificationDeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State(error) => write!(f, "notification delivery commit failed: {error}"),
        }
    }
}

impl std::error::Error for NotificationDeliveryError {}

fn canonicalize_with_existing_suffix(candidate: &Path) -> Result<PathBuf, String> {
    let mut existing = candidate;
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| "worktree path has no existing parent".to_string())?;
    }
    let canonical_existing = existing
        .canonicalize()
        .map_err(|error| format!("worktree path cannot be canonicalized: {error}"))?;
    let suffix = candidate
        .strip_prefix(existing)
        .map_err(|_| "worktree path cannot be resolved under project root".to_string())?;
    Ok(normalize_trailing_separator(
        canonical_existing.join(suffix),
    ))
}

fn normalize_trailing_separator(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if text.ends_with('/') && text != "/" {
        PathBuf::from(&text[..text.len() - 1])
    } else {
        path
    }
}

pub(crate) fn configured_project_key(root: &Path) -> Result<String, String> {
    let name = root
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    if name.is_empty() {
        return Err("cannot derive project key from empty project root".into());
    }
    let key = sanitize_identifier(name);
    if key.is_empty() || key.len() > 32 {
        return Err("project key must be a non-empty slug of at most 32 bytes".into());
    }
    Ok(key)
}

fn resolve_worktree_path(
    root: &Path,
    config: &crate::config::Config,
    raw: &str,
    stored_cleanup: bool,
) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("worktree path must be non-empty".into());
    }
    let path = Path::new(raw);
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("worktree path may not contain '..'".into());
    }
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let relative = raw.strip_prefix("./").unwrap_or(raw);
        root.join(relative)
    };
    let canonical_candidate = canonicalize_with_existing_suffix(&candidate)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("project root cannot be canonicalized: {error}"))?;
    let canonical_playground = canonical_root.join("playground");
    if stored_cleanup
        && canonical_candidate.starts_with(&canonical_playground)
        && canonical_candidate != canonical_playground
        && canonical_candidate
            .strip_prefix(&canonical_playground)
            .is_ok_and(|relative| relative.components().count() == 1)
    {
        return Ok(canonical_candidate);
    }
    if let Some(base_result) = config.worktree.canonical_base() {
        let base = base_result?;
        if base.starts_with(&canonical_root) {
            return Err("configured worktree base must be outside the project root".into());
        }
        let project_key = configured_project_key(&canonical_root)?;
        let task_slug = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| "worktree path must end in a valid UTF-8 task slug".to_string())?;
        let relative = config.worktree.render_relative(&project_key, task_slug)?;
        let expected = base.join(&relative);
        if !expected.starts_with(&base) {
            return Err("worktree path must match the configured worktree base/layout".into());
        }
        let canonical_expected = canonicalize_with_existing_suffix(&expected)?;
        if canonical_candidate == canonical_expected
            && canonical_candidate.starts_with(&base)
            && expected.starts_with(&base)
        {
            return Ok(canonical_candidate);
        }
        return Err("worktree path must match the configured worktree base/layout".into());
    }
    Err("new worktree requires a configured external worktree base".into())
}

fn validate_worktree_slug(raw: &str) -> Result<(), String> {
    let path = Path::new(raw);
    let leaf = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    if leaf.is_empty()
        || leaf.len() > 32
        || !leaf
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    {
        return Err("worktree basename must be a short slug (ASCII letters, digits, '.', '-' or '_'; max 32 chars)".into());
    }
    Ok(())
}

fn validate_worktree_reference(raw: &str) -> Result<(), String> {
    if raw.trim().is_empty() {
        return Err("worktree path must be non-empty".into());
    }
    validate_worktree_slug(raw)
}

fn validate_worktree_path(
    root: &Path,
    config: &crate::config::Config,
    raw: &str,
) -> Result<PathBuf, String> {
    validate_worktree_reference(raw)?;
    resolve_worktree_path(root, config, raw, false)
}

/// Normalize a declared task workspace. Register, relocate and scheduler
/// dispatch share this single path contract.
pub(crate) fn declare_task_workspace(
    root: &Path,
    config: &crate::config::Config,
    raw: &str,
) -> Result<String, String> {
    Ok(validate_worktree_path(root, config, raw)?
        .display()
        .to_string())
}

fn cleanup_worktree_path(
    root: &Path,
    config: &crate::config::Config,
    raw: &str,
) -> Result<PathBuf, String> {
    validate_worktree_reference(raw)?;
    resolve_worktree_path(root, config, raw, true)
}

fn task_claim_held(status: &str) -> bool {
    matches!(
        status,
        "working"
            | "blocked"
            | "verifying"
            | "reviewed"
            | "delivered"
            | "accepted"
            | "rework"
            | "merged"
    )
}

fn task_transition_allowed(current: &str, next: &str) -> bool {
    current == next
        || matches!(
            (current, next),
            ("pending", "cancelled")
                | ("working", "blocked" | "verifying" | "cancelled")
                | ("blocked", "working" | "cancelled")
                | (
                    "verifying",
                    "working" | "blocked" | "reviewed" | "cancelled"
                )
                | ("reviewed", "blocked" | "rework" | "cancelled")
                | ("rework", "working" | "blocked" | "verifying" | "cancelled")
                | ("delivered", "accepted" | "rework" | "cancelled")
                | ("accepted", "merged" | "rework" | "cancelled")
        )
}

fn task_delivery_allowed(status: &str) -> bool {
    matches!(status, "working" | "verifying" | "reviewed" | "rework")
}

pub struct Server {
    pub config: crate::config::Config,
    pub root: PathBuf,
    /// Project root used for worktree, identity and configuration checks.
    ///
    /// `storage_root` is separate because a host daemon may own more than one
    /// appserver route for the same project.  Those routes must not share a
    /// current-state file or mailbox projection. The resident route keeps both
    /// paths equal for backwards compatibility.
    pub storage_root: PathBuf,
    pub state_path: PathBuf,
    pub(crate) host_paths: HostPaths,
    pub state: Mutex<State>,
    pub appserver_candidate_check: Arc<AppServerCandidateCheck>,
    #[cfg(not(test))]
    pub tmux_notification_sink: Arc<TmuxNotificationSink>,
    pub appserver_notification_sink: Arc<TmuxNotificationSink>,
    pub appserver_thread_status: Arc<AppServerThreadStatus>,
    pub appserver_thread_archive: Arc<AppServerThreadArchive>,
    pub rex_monitor: rex_monitor::RexMonitor,
    pub mailbox_notify: Notify,
}

// Wire admission and the legacy handlers share the state reducer, but the
// admission check itself cannot hold `State` while a handler runs.  Keep a
// process-local gate per daemon root so a rebind cannot slip between those
// two phases.  This mutex is synchronization only; route and generation
// truth remains in the declared current control/state resources.
static WIRE_ROUTE_MUTATION_GATES: OnceLock<
    Mutex<std::collections::HashMap<PathBuf, Arc<Mutex<()>>>>,
> = OnceLock::new();

fn wire_route_mutation_gate(server: &Server) -> Arc<Mutex<()>> {
    let gates = WIRE_ROUTE_MUTATION_GATES.get_or_init(|| Mutex::new(Default::default()));
    let mut gates = gates.lock().unwrap();
    gates
        .entry(server.root.clone())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}
