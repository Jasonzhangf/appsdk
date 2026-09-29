
/// The resident v1 daemon owns one project reducer/journal.  Keep that
/// ownership explicit at the host boundary instead of treating the process
/// root as an implicit fallback for every valid project context.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostRouteOwner {
    ResidentProject { root: PathBuf, journal: PathBuf },
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
        let resident_journal = server
            .root
            .join(".agent-collab")
            .join("server")
            .join("journal.jsonl");
        // GlobalState is the only durable registration index available to the
        // v1 resident process.  A registered project without a resident
        // reducer remains visible as a route, but cannot be sent to this
        // project's State/journal until multi-project migration is complete.
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
                        journal: resident_journal.clone(),
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
const TASK_STATUSES: [&str; 12] = [
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
/// A new daemon must fence this writer before it replays the project journal;
/// otherwise an old binary could append concurrently under the new socket.
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
        |transport, source_thread_id, body, message_id, explicit, _mode| {
            if transport.kind != TransportKind::Tmux {
                return Err("TRANSPORT_UNSUPPORTED: Collab notifications require tmux".into());
            }
            let _ = (source_thread_id, explicit);
            let endpoint = transport.tmux_endpoint.as_ref().ok_or_else(|| {
                "TMUX_ENDPOINT_MISSING: selected transport has no endpoint".to_owned()
            })?;
            crate::client::adapters::tmux::notify(endpoint, message_id, body)
        },
    )
}

pub(crate) fn default_appserver_notification_sink() -> Arc<TmuxNotificationSink> {
    let tmux = default_tmux_notification_sink();
    Arc::new(
        move |transport, source_thread_id, body, message_id, explicit, mode| {
            if transport.kind != TransportKind::AppServer {
                tmux(
                    transport,
                    source_thread_id,
                    body,
                    message_id,
                    explicit,
                    mode,
                )
            } else if mode == "queued" {
                crate::client::adapters::codex_app_server::queued_notify(
                    transport,
                    source_thread_id,
                    body,
                    message_id,
                )
                .map_err(|error| error.to_string())
            } else {
                crate::client::adapters::codex_app_server::immediate_notify(
                    transport,
                    source_thread_id,
                    body,
                    message_id,
                )
                .map_err(|error| error.to_string())
            }
        },
    )
}

fn notification_sink(server: &Server) -> &Arc<TmuxNotificationSink> {
    &server.appserver_notification_sink
}

fn default_appserver_thread_status() -> Arc<AppServerThreadStatus> {
    Arc::new(|transport, thread_id| {
        if transport.kind == TransportKind::Tmux {
            let endpoint = transport.tmux_endpoint.as_ref().ok_or_else(|| {
                "TMUX_ENDPOINT_MISSING: selected transport has no endpoint".to_owned()
            })?;
            if endpoint.pane_id != thread_id {
                return Err(
                    "TMUX_ENDPOINT_MISMATCH: requested pane does not match selected endpoint"
                        .into(),
                );
            }
            return crate::client::adapters::tmux::view(endpoint);
        }
        crate::client::adapters::codex_app_server::read_thread_status(transport, thread_id)
            .map_err(|error| error.to_string())
    })
}

pub(crate) fn default_appserver_thread_archive() -> Arc<AppServerThreadArchive> {
    Arc::new(|_transport, _thread_id| {
        Err("TRANSPORT_UNSUPPORTED: tmux has no Codex thread archive operation".into())
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
    const HEX: &[u8; 16] = b"0123456789abcdef";
    const CHUNK_BYTES: usize = 96;
    let bytes = app_scope.as_bytes();
    let mut path = root.join(".agent-collab").join("server").join("runtimes");
    for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let prefix = if index == 0 {
            format!("v1-{}-", bytes.len())
        } else {
            String::new()
        };
        let mut component = String::with_capacity(prefix.len() + chunk.len() * 2);
        component.push_str(&prefix);
        for byte in chunk {
            component.push(HEX[(byte >> 4) as usize] as char);
            component.push(HEX[(byte & 0x0f) as usize] as char);
        }
        path.push(component);
    }
    path
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum CommandJournalFault {
    StartAppend = 1,
    StartSync = 2,
    CompletionAppend = 3,
    CompletionSync = 4,
}

#[cfg(test)]
thread_local! {
    static COMMAND_JOURNAL_FAULT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn inject_command_journal_fault(fault: CommandJournalFault) {
    COMMAND_JOURNAL_FAULT.with(|injected| injected.set(fault as u8));
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum SubagentJournalFault {
    StartAppend = 11,
    StartSync = 12,
    CloseFirstAppend = 21,
    CloseFirstSync = 22,
    CloseFinalAppend = 31,
    CloseFinalSync = 32,
    WorkingAppend = 41,
    WorkingSync = 42,
}

#[cfg(test)]
thread_local! {
    static SUBAGENT_JOURNAL_FAULT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn inject_subagent_journal_fault(fault: SubagentJournalFault) {
    SUBAGENT_JOURNAL_FAULT.with(|injected| injected.set(fault as u8));
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum TaskRegisterJournalFault {
    Append = 51,
    Sync = 52,
}

#[cfg(test)]
thread_local! {
    static TASK_REGISTER_JOURNAL_FAULT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn inject_task_register_journal_fault(fault: TaskRegisterJournalFault) {
    TASK_REGISTER_JOURNAL_FAULT.with(|injected| injected.set(fault as u8));
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum CurrentThreadRouteJournalFault {
    Append,
    Sync,
}

#[cfg(test)]
thread_local! {
    static CURRENT_THREAD_ROUTE_JOURNAL_FAULT: std::cell::Cell<Option<CurrentThreadRouteJournalFault>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn inject_current_thread_route_journal_fault(fault: CurrentThreadRouteJournalFault) {
    CURRENT_THREAD_ROUTE_JOURNAL_FAULT.with(|injected| injected.set(Some(fault)));
}

#[derive(Clone, Copy)]
enum CommandJournalPhase {
    Start,
    Business,
    Completion,
}

#[cfg(test)]
const DIRECT_MESSAGE_WAKE_COOLDOWN_MS: i64 = 60_000;

#[derive(Debug)]
enum NotificationDeliveryError {
    Journal(notification_contract::JournalError),
}

#[derive(Debug)]
enum NotificationAttempt {
    Accepted,
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
            Self::Journal(error) => write!(f, "notification delivery commit failed: {error}"),
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
    Ok(canonical_existing.join(suffix))
}

fn configured_project_key(root: &Path) -> Result<String, String> {
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
    if canonical_candidate.starts_with(&canonical_playground)
        && canonical_candidate != canonical_playground
    {
        return Ok(canonical_candidate);
    }
    if let Some(base_result) = config.worktree.canonical_base() {
        let base = base_result?;
        let project_key = configured_project_key(&canonical_root)?;
        let task_slug = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| "worktree path must end in a valid task slug".to_string())?;
        let relative = config.worktree.render_relative(&project_key, task_slug)?;
        let expected = base.join(&relative);
        if canonical_candidate == canonicalize_with_existing_suffix(&expected)? {
            return Ok(canonical_candidate);
        }
        return Err("worktree path must match the configured worktree base/layout".into());
    }
    Err("worktree path must be inside ./playground".into())
}

fn validate_worktree_path(
    root: &Path,
    config: &crate::config::Config,
    raw: &str,
) -> Result<PathBuf, String> {
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
    resolve_worktree_path(root, config, raw)
}

fn cleanup_worktree_path(
    root: &Path,
    config: &crate::config::Config,
    raw: &str,
) -> Result<PathBuf, String> {
    resolve_worktree_path(root, config, raw)
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
            ("working", "blocked" | "verifying" | "cancelled")
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
    /// reducer journal or mailbox projection.  The resident route keeps both
    /// paths equal for backwards compatibility.
    pub storage_root: PathBuf,
    pub journal_path: PathBuf,
    pub(crate) host_paths: HostPaths,
    pub state: Mutex<State>,
    pub journal: Mutex<std::fs::File>,
    pub appserver_candidate_check: Arc<AppServerCandidateCheck>,
    #[cfg(not(test))]
    pub tmux_notification_sink: Arc<TmuxNotificationSink>,
    pub appserver_notification_sink: Arc<TmuxNotificationSink>,
    pub appserver_thread_status: Arc<AppServerThreadStatus>,
    pub appserver_thread_archive: Arc<AppServerThreadArchive>,
    pub mailbox_notify: Notify,
}

// Wire admission and the legacy handlers share the state reducer, but the
// admission check itself cannot hold `State` while a handler runs.  Keep a
// process-local gate per daemon root so a rebind cannot slip between those
// two phases.  This mutex is synchronization only; route and generation
// truth remains in `State`/the journal.
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

fn record_activity(root: &Path, kind: &str, detail: serde_json::Value) -> Result<(), String> {
    let path = root.join(".agent-collab/server/events.jsonl");
    let record = json!({
        "ts": now_ms(),
        "kind": kind,
        "detail": detail,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open events: {error}"))?;
    use std::io::Write;
    let mut line =
        serde_json::to_vec(&record).map_err(|error| format!("serialize events: {error}"))?;
    line.push(b'\n');
    file.write_all(&line)
        .map_err(|error| format!("append events: {error}"))
}

fn request_activity(req: &Req, resp: &Resp) -> serde_json::Value {
    let mut request = serde_json::to_value(req).unwrap_or_else(|_| json!({}));
    if let Some(obj) = request.as_object_mut() {
        obj.remove("token");
        obj.remove("launch_env");
    }
    json!({
        "op": request.get("op").cloned().unwrap_or(json!("unknown")),
        "actor": request.get("worker_id").or_else(|| request.get("from")).cloned(),
        "task_id": request.get("task_id").cloned(),
        "target": request.get("to").cloned(),
        "ok": resp.ok,
        "error": resp.error,
        "request": request,
    })
}
