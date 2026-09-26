mod client;
mod config;
mod identity;
mod install_skills;
pub(crate) mod migration;
mod proto;
mod reset;
mod scope;
mod server;
mod subagent;

use clap::{Parser, Subcommand};
use identity::{AppServerId, CommandId, Identity, OperationId, RuntimeIdentity};
use proto::{ProjectContext, Req, Resp, TransportKind};
use scope::Scope;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LIVE_CLOSURE_TIMEOUT_MS_ENV: &str = "COLLAB_LIVE_CLOSURE_TIMEOUT_MS";
const DEFAULT_LIVE_CLOSURE_TIMEOUT_MS: u64 = 180_000;
const MAX_LIVE_CLOSURE_TIMEOUT_MS: u64 = 3_600_000;

fn live_closure_timeout_from_value(value: Option<&str>) -> anyhow::Result<Duration> {
    let milliseconds = match value {
        None => DEFAULT_LIVE_CLOSURE_TIMEOUT_MS,
        Some(value) => value.parse::<u64>().map_err(|error| {
            anyhow::anyhow!(
                "COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID:{LIVE_CLOSURE_TIMEOUT_MS_ENV}:{error}"
            )
        })?,
    };
    if milliseconds == 0 || milliseconds > MAX_LIVE_CLOSURE_TIMEOUT_MS {
        anyhow::bail!(
            "COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID:{LIVE_CLOSURE_TIMEOUT_MS_ENV}:must_be_between_1_and_{MAX_LIVE_CLOSURE_TIMEOUT_MS}_milliseconds"
        );
    }
    Ok(Duration::from_millis(milliseconds))
}

fn live_closure_timeout() -> anyhow::Result<Duration> {
    live_closure_timeout_from_value(std::env::var(LIVE_CLOSURE_TIMEOUT_MS_ENV).ok().as_deref())
}

fn live_closure_receipt_consumed(
    receipt: &serde_json::Value,
    message_id: &str,
    challenge: &str,
) -> bool {
    receipt.get("id").and_then(serde_json::Value::as_str) == Some(message_id)
        && receipt.get("state").and_then(serde_json::Value::as_str) == Some("read")
        && receipt.get("body").and_then(serde_json::Value::as_str) == Some(challenge)
}

fn wait_live_closure_receipt<F>(
    mut read: F,
    deadline: Instant,
    message_id: &str,
    challenge: &str,
) -> anyhow::Result<serde_json::Value>
where
    F: FnMut() -> anyhow::Result<serde_json::Value>,
{
    loop {
        let receipt = read()?;
        if live_closure_receipt_consumed(&receipt, message_id, challenge) {
            return Ok(receipt);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_RECEIPT_NOT_CONSUMED");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Parser)]
#[command(
    name = "collab",
    version = env!("COLLAB_VERSION"),
    about = "Project-local coordination for multi-agent work"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Managed persistent agent peers (current project only)
    Subagent {
        #[command(subcommand)]
        command: subagent::Action,
    },
    /// Show the effective policy from ~/.appsdk/config.toml
    Config,
    /// Create .agent-collab skeleton in the current directory
    Init {
        /// Explicitly select or create this peer identity when the current
        /// runtime cannot be recovered from its existing session/thread anchors.
        #[arg(long)]
        worker_id: Option<String>,
    },
    /// Hidden: daemon entrypoint (spawned by `up`)
    #[command(hide = true)]
    Serve,
    /// Start the coordination daemon (idempotent)
    Up,
    /// Explicitly stop the daemon and disable automatic restart
    Down,
    /// Show server summary (pass --all to aggregate all workers, tasks, subagents)
    Status {
        #[arg(long)]
        all: bool,
    },
    /// Inspect or read messages from durable mailbox
    Mailbox {
        #[command(subcommand)]
        cmd: MailboxCmd,
    },
    /// Deprecated: declared roles were removed
    Role,
    /// List registered peers and their local activity projection
    /// (does not report live master authority; use `collab master status`)
    Who,
    /// Inspect or explicitly assign collab master authority
    Master {
        #[command(subcommand)]
        command: MasterCmd,
    },
    /// Resolve the daemon-owned route for a tmux session and pane
    Route {
        #[command(subcommand)]
        command: RouteCmd,
    },
    /// Run one bounded, authenticated live-closure path probe.
    LiveClosure {
        #[command(subcommand)]
        command: LiveClosureCmd,
    },
    /// Hidden alias: previous collab root commands are collab master
    #[command(hide = true)]
    Root {
        #[command(subcommand)]
        command: MasterCmd,
    },
    /// Refresh this worker's App Server registration
    Worker {
        #[command(subcommand)]
        cmd: WorkerCmd,
    },
    /// Deprecated: permanent master role was removed
    TransferMaster { target: String },
    /// Deprecated: use explicit lifecycle cleanup or daemon migration tooling
    RemoveWorker {
        target: String,
        #[arg(long)]
        force: bool,
    },
    /// Retire the legacy project-local Collab control plane and rebuild the
    /// current empty baseline. Offline, explicit authorization, transactional.
    Reset {
        /// Explicit operator authorization text; required.
        #[arg(long)]
        approval: Option<String>,
        /// Confirm that the named legacy control plane may be discarded.
        #[arg(long)]
        discard_legacy: bool,
    },
    /// Get or create your worker identity and bind the Codex thread
    Whoami {
        #[arg(long)]
        worker: Option<String>,
    },
    /// Send a message to another worker
    #[command(alias = "sendmessage")]
    Send {
        /// Sender identity; defaults to current collab identity, COLLAB_WORKER, or 'operator'
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: String,
        /// Short topic shown in the notification preview
        #[arg(long)]
        subject: String,
        #[arg(long, default_value = "notify")]
        r#type: String,
        #[arg(long)]
        in_reply_to: Option<String>,
        #[arg(long, default_value = "immediate")]
        delivery: String,
        #[arg(trailing_var_arg = true)]
        body: Vec<String>,
    },
    /// Discover and explicitly subscribe to finite notifications
    Notify {
        #[command(subcommand)]
        cmd: NotifyCmd,
    },
    /// Block until messages arrive (long-poll)
    Recv {
        #[arg(long, default_value_t = 600)]
        timeout: u64,
        /// act as another registered worker (testing / delegated runs)
        #[arg(long)]
        worker: Option<String>,
        /// Replay one committed receive identity instead of consuming new mail
        #[arg(long = "receive-id")]
        receive_id: Option<String>,
    },
    /// List unread inbox
    Inbox {
        #[arg(long)]
        worker: Option<String>,
    },
    /// Single agent bootstrap: resolve root, start daemon, restore identity and
    /// registration, re-arm default notify, then return the authoritative snapshot
    Context {
        #[arg(long)]
        worker: Option<String>,
    },
    /// Mark messages as read
    Ack {
        ids: Vec<String>,
        #[arg(long)]
        worker: Option<String>,
        /// Acknowledge all pending and delivered messages in inbox
        #[arg(long)]
        all: bool,
    },
    /// Query message status (wake attempts, answered)
    Msg { msg_id: String },
    /// Task registration and lifecycle (task owner owns feature/worktree)
    Task {
        #[command(subcommand)]
        cmd: TaskCmd,
    },
    /// Inspect, plan, apply, and verify an existing-project migration
    Migrate {
        #[command(subcommand)]
        cmd: MigrateCmd,
    },
    /// Install the embedded collab skill bundle into a global skills
    /// directory. Default target is `~/.agents/skills/collab`; pass
    /// `--target` to override. Existing files are skipped unless
    /// `--force` is given.
    InstallSkills {
        /// Destination directory for the collab skill bundle.
        /// Defaults to `~/.agents/skills/collab`.
        #[arg(long)]
        target: Option<std::path::PathBuf>,
        /// Overwrite existing files in the target instead of skipping them.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum NotifyCmd {
    /// List supported notification methods and events
    Methods,
    /// Register one finite notification subscription (direct-message is reusable)
    Subscribe {
        #[arg(long)]
        event: String,
        #[arg(long)]
        subject: Option<String>,
        /// Absolute UTC epoch milliseconds; repeat to define multiple fire times
        #[arg(long = "at-ms")]
        at_ms: Vec<i64>,
        /// Period in milliseconds; mutually exclusive with --at-ms
        #[arg(long = "every-ms")]
        every_ms: Option<i64>,
        /// Total number of notifications for a periodic subscription (1..=100)
        #[arg(long, default_value_t = 1)]
        repeat_count: u32,
        #[arg(long)]
        trigger_ms: Option<i64>,
        #[arg(long)]
        ttl_seconds: u64,
    },
    /// List the caller's notification subscriptions
    Status,
    /// Cancel one caller-owned notification subscription
    Unsubscribe { subscription_id: String },
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Register a task owned by the calling peer
    Register {
        id: String,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        feature: Option<String>,
        #[arg(long)]
        worktree: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        base_commit: Option<String>,
        /// Owner-local priority: p0 (highest) through p4
        #[arg(long)]
        priority: Option<String>,
        /// Next lifecycle step for the task owner
        #[arg(long)]
        next: Option<String>,
        /// Complete /goal prompt; must begin with /goal and contains no wrapper text
        #[arg(long)]
        goal: Option<String>,
    },
    /// Relocate the caller's task to a short playground worktree
    Relocate {
        id: String,
        #[arg(long)]
        worktree: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        base_commit: Option<String>,
    },
    /// Update task status/next step by its owner
    Update {
        id: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        next: Option<String>,
    },
    /// Accept an assigned task and atomically begin owner execution
    Accept { id: String },
    /// Deprecated: peers self-register tasks; no central available queue
    Claim { id: String },
    /// Put an owned task into resource-waiting state until another task releases
    Wait {
        id: String,
        #[arg(long = "for")]
        blocking_task: String,
    },
    /// Record owner-local delivery evidence before integration
    Deliver {
        id: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        worktree: String,
    },
    /// Accept a delivered task or return it for rework
    Review {
        id: String,
        #[arg(long, conflicts_with = "rework", required_unless_present = "rework")]
        accept: bool,
        #[arg(long, conflicts_with = "accept", required_unless_present = "accept")]
        rework: bool,
        #[arg(long)]
        evidence: String,
    },
    /// Record exact integration of an accepted task on main
    Integrated {
        id: String,
        #[arg(long)]
        commit: String,
        #[arg(long)]
        evidence: String,
    },
    /// Mark the caller's task blocked without notifying unrelated peers
    Block {
        id: String,
        #[arg(long)]
        next: Option<String>,
    },
    /// Complete the cleanup obligation left by a forced close.
    ///
    /// Verifies and removes the task's declared worktree/branch under the same
    /// contract as a normal close, refuses while another non-closed task
    /// references the same worktree or branch, replaces the unverified receipt
    /// with verified evidence, stops the owner's automatic lease once its last
    /// responsibility is verified, and releases dependent waiters exactly once.
    /// A retry resumes the remaining release instead of double-releasing.
    FinalizeCleanup { id: String },
    /// Close a merged task and clean up its declared worktree/branch.
    /// With --force the live master may close any task. With no live master,
    /// the owner may close its task, or a registered peer may close an
    /// orphaned task after the owner's App Server identity is lost. Force close
    /// stops keepalives without deleting the worktree or branch and requires
    /// a non-empty --reason.
    Close {
        id: String,
        #[arg(long)]
        force: bool,
        #[arg(long = "reason")]
        reason: Option<String>,
    },
    /// Deprecated: peers self-register tasks; no central dispatch
    Dispatch,
    /// Show task registry
    Status { id: Option<String> },
}

#[derive(Subcommand, Debug, Clone)]
pub enum MailboxCmd {
    /// Read messages in chronological order
    Read {
        /// Include all messages across the project mailbox
        #[arg(long)]
        all: bool,
        /// Sorting order: time-asc (default) or time-desc
        #[arg(long, default_value = "time-asc")]
        sort: String,
        /// Filter messages by specific worker ID
        #[arg(long)]
        worker: Option<String>,
    },
}

#[derive(Subcommand)]
enum MasterCmd {
    /// Promote this peer when no live master exists; requires the user's approval text
    Promote {
        #[arg(long)]
        approval: String,
    },
    /// Delegate master authority to another registered peer (live master only)
    Delegate { target: String },
    /// Send a durable message to the master of another explicit project
    Send {
        #[arg(long)]
        project: std::path::PathBuf,
        #[arg(long)]
        to: String,
        #[arg(long)]
        subject: String,
        #[arg(trailing_var_arg = true)]
        body: Vec<String>,
    },
    /// Show the current live master, if any
    Status,
    /// Deprecated: permanent master recovery was removed
    Recover,
}

#[derive(Subcommand)]
enum RouteCmd {
    /// Resolve the route bound to the current tmux session and pane
    Resolve {
        #[arg(long = "tmux-session-id")]
        session_id: Option<String>,
        #[arg(long = "pane-id")]
        pane_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum LiveClosureCmd {
    /// Send one challenge-bound message and observe target execution/consume.
    /// The command fails closed unless the durable receive receipt binds to
    /// the same challenge and message ID.
    Probe {
        #[arg(long)]
        closure_id: String,
        #[arg(long)]
        source_commit: String,
        #[arg(long)]
        artifact_hash: String,
        #[arg(long)]
        environment_id: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        to: String,
        /// Explicit target project for the independently authenticated master
        /// in a cross-project master-to-master probe.
        #[arg(long)]
        to_project: Option<std::path::PathBuf>,
    },
    /// Read one authenticated target route and observe its native execution.
    Observe {
        #[arg(long)]
        to: String,
        #[arg(long)]
        challenge: String,
        #[arg(long)]
        message_id: String,
    },
}

#[derive(Subcommand)]
enum WorkerCmd {
    /// Re-register the current App Server thread without changing task ownership
    Recover,
    /// Inspect worker status (liveness, identity, agent state, unacked notifications)
    Status {
        /// Optional worker ID to inspect (defaults to all registered workers)
        id: Option<String>,
    },
    /// Capture durable App Server thread evidence before closing a worker
    Snapshot {
        /// Worker ID to inspect
        id: String,
        /// Maximum number of recent thread items to read
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },
    /// Live master retires a worker registration
    Close {
        /// Worker ID to close
        id: String,
        /// Why this worker is being closed; recorded for audit
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum MigrateCmd {
    /// Inspect current durable state and migration blockers
    Inspect,
    /// Create a migration plan; does not freeze admission
    Plan,
    /// Freeze task admission and persist a deterministic snapshot
    Apply,
    /// Verify replayed state and resume task admission
    Verify,
}

fn out<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

/// Register an identity with the server (idempotent for the same token).
fn register(scope: &Scope, ident: &mut Identity) -> anyhow::Result<serde_json::Value> {
    let context_runtime = match ident.runtime.as_ref() {
        Some(runtime) => {
            runtime.validate()?;
            runtime.clone()
        }
        None => RuntimeIdentity::cli_adapter(&ident.worker_id)?,
    };
    register_with_runtime(scope, ident, context_runtime)
}

/// Bootstrap recovery with the only runtime identity accepted before the
/// daemon has restored this worker's registered route.
fn register_recovery(scope: &Scope, ident: &mut Identity) -> anyhow::Result<serde_json::Value> {
    register_with_runtime(
        scope,
        ident,
        RuntimeIdentity::cli_adapter(&ident.worker_id)?,
    )
}

fn register_with_runtime(
    scope: &Scope,
    ident: &mut Identity,
    context_runtime: RuntimeIdentity,
) -> anyhow::Result<serde_json::Value> {
    let cwd = scope.root.display().to_string();
    let response: serde_json::Value = client::call_with_runtime_identity_at_root_daemon(
        &scope.sock_path(),
        &Req::Register {
            worker_id: ident.worker_id.clone(),
            token: ident.token.clone(),
            cwd,
            candidates: Some(proto::TransportCandidates {
                appserver: crate::client::adapters::candidate_from_env()
                    .map_err(anyhow::Error::msg)?,
                tmux: crate::client::adapters::tmux::candidate_from_env().ok(),
            }),
        },
        &scope.root,
        &context_runtime,
    )?;
    let (runtime, transport) =
        identity::registration_from_receipt(&response, &ident.worker_id, &scope.root)?;
    if runtime.appserver_id != context_runtime.appserver_id {
        anyhow::bail!(
            "registration receipt app scope mismatch: expected {}, observed {}",
            context_runtime.appserver_id,
            runtime.appserver_id
        );
    }
    identity::persist_registration(scope, ident, runtime, transport)?;
    Ok(response)
}

/// Identity bootstrap used by every command that acts as a worker.
fn me(scope: &Scope, worker: Option<String>) -> anyhow::Result<Identity> {
    let worker = worker.or_else(|| std::env::var("COLLAB_WORKER").ok());
    let mut ident = identity::load_or_create(scope, worker, None)?;
    if ident.runtime.is_none() || ident.transport.is_none() {
        let _ = register(scope, &mut ident)?;
    } else if !persisted_runtime_matches_scope(scope, &ident)? {
        // A persisted binding that no longer matches this session, thread, or
        // canonical cwd must not be reused: commands would then be dispatched
        // under a stale App Server address.  Re-register through the same
        // owner used by `ensure_registration`.
        register_recovery(scope, &mut ident)?;
    }
    runtime_for_request(&ident)?;
    Ok(ident)
}

/// How `collab context` changed an identity during automatic bootstrap.
enum RegistrationOutcome {
    Created,
    Reused,
    Recovered,
    Recreated,
}

fn ensure_registration(scope: &Scope, ident: &mut Identity) -> anyhow::Result<serde_json::Value> {
    ensure_registration_with_outcome(scope, ident).map(|(value, _)| value)
}

fn ensure_registration_with_outcome(
    scope: &Scope,
    ident: &mut Identity,
) -> anyhow::Result<(serde_json::Value, RegistrationOutcome)> {
    if ident.runtime.is_none() || ident.transport.is_none() {
        let value = register(scope, ident)?;
        Ok((value, RegistrationOutcome::Created))
    } else if !persisted_runtime_matches_scope(scope, ident)? {
        match register_recovery(scope, ident) {
            Ok(value) => Ok((value, RegistrationOutcome::Recovered)),
            Err(_error) => {
                // A stale binding that can no longer be recovered in place.
                // Re-register fresh with the same worker token so the server
                // supersedes the old transport; the previous identity is
                // dropped as the active peer without a daemon restart.
                ident.runtime = None;
                ident.transport = None;
                let value = register(scope, ident)?;
                Ok((value, RegistrationOutcome::Recreated))
            }
        }
    } else {
        runtime_for_request(ident)?;
        Ok((json!({"reused": true}), RegistrationOutcome::Reused))
    }
}

fn persisted_runtime_matches_scope(scope: &Scope, ident: &Identity) -> anyhow::Result<bool> {
    let runtime = ident
        .runtime
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("identity has no registered runtime binding"))?;
    // A missing transport cannot be reused, but it is a normal legacy shape
    // rather than an error: report "not reusable" so the caller re-registers.
    let Some(transport) = ident.transport.as_ref() else {
        return Ok(false);
    };
    if transport.kind == TransportKind::Tmux {
        let Some(persisted_endpoint) = transport.tmux_endpoint.as_ref() else {
            return Ok(false);
        };
        let Ok(current) = crate::client::adapters::tmux::candidate_from_env() else {
            return Ok(false);
        };
        if !crate::client::adapters::tmux::same_pane_route(&current.endpoint, persisted_endpoint) {
            return Ok(false);
        }
    } else if runtime.native_thread_id.is_some() {
        let session = std::env::var("CODEX_SESSION_ID").ok();
        let thread = std::env::var("CODEX_THREAD_ID").ok();
        if let (Some(session), Some(thread)) = (session, thread) {
            let persisted_thread = runtime
                .native_thread_id
                .as_ref()
                .map(crate::identity::NativeThreadId::as_str);
            let persisted_session = runtime
                .session_id
                .as_ref()
                .map(crate::identity::SessionId::as_str);
            if persisted_thread != Some(thread.as_str())
                || persisted_session != Some(session.as_str())
            {
                return Ok(false);
            }
        }
    }
    let host_paths = scope.host_paths()?;
    let scope_root = std::fs::canonicalize(&scope.root)?;
    if ident
        .project_scope
        .as_ref()
        .is_none_or(|project_scope| project_scope.as_str() != scope_root.to_string_lossy())
    {
        return Ok(false);
    }
    Ok(
        scope::canonical_route_for_identity(&host_paths, &scope.root, &runtime.appserver_id)
            .is_ok_and(|route| route.root == scope_root),
    )
    .and_then(|file_says_reusable| {
        if !file_says_reusable {
            return Ok(false);
        }
        // The route journal on disk is not the live truth: after a daemon
        // restart or re-registration the running daemon may hold no route for
        // this address even though the file still lists one.  Reuse is only
        // valid when the live daemon resolves the same session/thread.
        match ident.transport.as_ref() {
            Some(transport) if transport.kind == crate::proto::TransportKind::Tmux => {
                let Some(endpoint) = transport.tmux_endpoint.as_ref() else {
                    return Ok(false);
                };
                Ok(client::resolve_route(&scope.sock_path(), endpoint).is_ok())
            }
            Some(transport) if transport.kind == crate::proto::TransportKind::AppServer => {
                let (Some(session), Some(thread)) = (
                    runtime
                        .session_id
                        .as_ref()
                        .map(crate::identity::SessionId::as_str),
                    runtime
                        .native_thread_id
                        .as_ref()
                        .map(crate::identity::NativeThreadId::as_str),
                ) else {
                    return Ok(false);
                };
                if transport.thread_id.as_deref() != Some(thread)
                    || transport.session_id.as_deref() != Some(session)
                {
                    return Ok(false);
                }
                // The persisted route file is not live truth: after a daemon
                // restart or re-registration the running daemon may hold no
                // route for this address. Reuse is only valid when the live
                // daemon resolves the same session/thread.
                Ok(client::resolve_native_route(&scope.sock_path(), session, thread).is_ok())
            }
            _ => Ok(false),
        }
    })
}

fn runtime_for_request<'a>(ident: &'a Identity) -> anyhow::Result<&'a RuntimeIdentity> {
    let runtime = ident.runtime.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "identity has no registered runtime binding; register the current peer before making a project request"
        )
    })?;
    runtime.validate()?;
    if runtime.agent_id.as_str() != ident.worker_id {
        anyhow::bail!(
            "runtime binding agent does not match identity worker: expected {}, observed {}",
            ident.worker_id,
            runtime.agent_id
        );
    }
    Ok(runtime)
}

fn registered_runtime_projection(
    scope: &Scope,
    ident: &Identity,
    daemon_pid: u32,
) -> anyhow::Result<serde_json::Value> {
    let runtime = ident
        .runtime
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("registered identity is missing its runtime"))?;
    let transport = ident
        .transport
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("registered identity is missing its transport"))?;
    if daemon_pid == 0 {
        anyhow::bail!("registered daemon PID is zero");
    }
    let project_root = std::fs::canonicalize(&scope.root)?;
    match transport.kind {
        crate::proto::TransportKind::Tmux => {
            let endpoint = transport.tmux_endpoint.as_ref().ok_or_else(|| {
                anyhow::anyhow!("registered tmux transport is missing its endpoint")
            })?;
            let expected_session = endpoint
                .codex_session_id
                .as_deref()
                .unwrap_or(&endpoint.tmux_session_id);
            let expected_thread = endpoint
                .codex_thread_id
                .as_deref()
                .unwrap_or(&endpoint.pane_id);
            if runtime
                .session_id
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                != Some(expected_session)
                || runtime
                    .native_thread_id
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    != Some(expected_thread)
            {
                anyhow::bail!("registered tmux endpoint does not match its runtime route");
            }
            Ok(json!({
                "runtimeId": runtime.runtime_id,
                "appserverId": runtime.appserver_id,
                "transport": "tmux",
                "tmuxEndpoint": endpoint,
                "projectRoot": project_root,
                "capabilities": transport.capabilities,
                "processId": daemon_pid,
            }))
        }
        crate::proto::TransportKind::AppServer => {
            let session = transport.session_id.as_deref().ok_or_else(|| {
                anyhow::anyhow!("registered App Server transport is missing its session id")
            })?;
            let thread = transport.thread_id.as_deref().ok_or_else(|| {
                anyhow::anyhow!("registered App Server transport is missing its thread id")
            })?;
            if runtime
                .session_id
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                != Some(session)
                || runtime
                    .native_thread_id
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    != Some(thread)
            {
                anyhow::bail!("registered App Server transport does not match its runtime route");
            }
            Ok(json!({
                "runtimeId": runtime.runtime_id,
                "appserverId": runtime.appserver_id,
                "transport": "appserver",
                "endpoint": transport.endpoint,
                "namespace": transport.namespace,
                "sessionId": session,
                "threadId": thread,
                "projectRoot": project_root,
                "capabilities": transport.capabilities,
                "processId": daemon_pid,
            }))
        }
    }
}

fn call_project<T: DeserializeOwned>(
    scope: &Scope,
    ident: &Identity,
    request: &Req,
) -> anyhow::Result<T> {
    let runtime = runtime_for_request(ident)?;
    client::call_with_runtime_identity_at_root(&scope.sock_path(), request, &scope.root, runtime)
}

#[derive(Debug)]
#[cfg(test)]
struct LiveClosureExpectedNativeInputs {
    exact: Vec<String>,
    batch_category: String,
}

#[cfg(test)]
fn live_closure_item_text(item: &serde_json::Value) -> Option<&str> {
    let payload = live_closure_item_payload(item);
    let item_type = payload.get("type").and_then(serde_json::Value::as_str);
    if !matches!(item_type, Some("userMessage") | Some("user_message")) {
        return None;
    }
    let content = payload
        .get("content")
        .and_then(serde_json::Value::as_array)?;
    if content.len() != 1
        || content[0].get("type").and_then(serde_json::Value::as_str) != Some("text")
    {
        return None;
    }
    content[0].get("text").and_then(serde_json::Value::as_str)
}

#[cfg(test)]
fn live_closure_item_contains_challenge(item: &serde_json::Value, challenge: &str) -> bool {
    live_closure_item_text(item) == Some(challenge)
}

#[cfg(test)]
fn live_closure_item_payload(item: &serde_json::Value) -> &serde_json::Value {
    item.get("item").unwrap_or(item)
}

#[cfg(test)]
fn live_closure_item_turn_id(item: &serde_json::Value) -> Option<&str> {
    let turn_id = item
        .get("turnId")
        .or_else(|| item.get("turn_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty());
    if turn_id.is_some() {
        return turn_id;
    }
    let payload = item.get("item")?;
    payload
        .get("turnId")
        .or_else(|| payload.get("turn_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
fn live_closure_item_message_id(item: &serde_json::Value) -> Option<&str> {
    let payload = live_closure_item_payload(item);
    payload
        .get("clientUserMessageId")
        .or_else(|| payload.get("clientId"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.strip_prefix("collab-notification-").or(Some(value)))
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
fn live_closure_function_call_output_fields(item: &serde_json::Value) -> Option<(&str, &str)> {
    let payload = live_closure_item_payload(item);
    if payload.get("type").and_then(serde_json::Value::as_str) != Some("functionCallOutput")
        || payload.get("name").and_then(serde_json::Value::as_str) != Some("send_message_to_thread")
    {
        return None;
    }
    live_closure_item_turn_id(item)?;
    let output = payload.get("output").and_then(serde_json::Value::as_str)?;
    let mut lines = output.lines();
    if lines.next()? != "<codex_delegation>" {
        return None;
    }
    let mut next_line = lines.next()?;
    if let Some(source_thread_id) = next_line
        .strip_prefix("  <source_thread_id>")
        .and_then(|value| value.strip_suffix("</source_thread_id>"))
    {
        if source_thread_id.is_empty()
            || source_thread_id.trim() != source_thread_id
            || source_thread_id.contains('<')
            || source_thread_id.contains('>')
            || source_thread_id.contains('&')
        {
            return None;
        }
        next_line = lines.next()?;
    }
    let client_message_id = next_line
        .strip_prefix("  <client_message_id>")?
        .strip_suffix("</client_message_id>")?;
    let client_message_id = client_message_id
        .strip_prefix("collab-notification-")
        .unwrap_or(client_message_id);
    if client_message_id.is_empty()
        || client_message_id.trim() != client_message_id
        || client_message_id.contains('<')
        || client_message_id.contains('>')
        || client_message_id.contains('&')
    {
        return None;
    }
    let challenge = lines
        .next()?
        .strip_prefix("  <input>")?
        .strip_suffix("</input>")?;
    if challenge.is_empty() || challenge.trim() != challenge {
        return None;
    }
    if lines.next()? != "</codex_delegation>" || lines.next().is_some() {
        return None;
    }
    Some((client_message_id, challenge))
}

#[cfg(test)]
fn live_closure_item_matches_input(
    item: &serde_json::Value,
    expected_inputs: &LiveClosureExpectedNativeInputs,
    message_id: &str,
) -> bool {
    live_closure_item_turn_id(item).is_some()
        && (live_closure_item_text(item).is_some_and(|text| {
            (live_closure_item_message_id(item) == Some(message_id)
                && expected_inputs
                    .exact
                    .iter()
                    .any(|expected_input| text == expected_input))
                || live_closure_batch_input_contains_message(
                    text,
                    message_id,
                    &expected_inputs.batch_category,
                    false,
                )
        }) || live_closure_function_call_output_fields(item).is_some_and(
            |(observed_message_id, observed)| {
                (observed_message_id == message_id
                    && expected_inputs.exact.iter().any(|expected_input| {
                        observed
                            == client::adapters::codex_app_server::escape_delegated_text(
                                expected_input,
                            )
                    }))
                    || live_closure_batch_input_contains_message(
                        observed,
                        message_id,
                        &expected_inputs.batch_category,
                        true,
                    )
            },
        ))
}

#[cfg(test)]
fn live_closure_expected_native_inputs(
    receipt: &serde_json::Value,
    message_id: &str,
    challenge: &str,
) -> anyhow::Result<LiveClosureExpectedNativeInputs> {
    let receipt_type = receipt
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let supported_type = matches!(receipt_type, "notify" | "notification");
    if receipt.get("id").and_then(serde_json::Value::as_str) != Some(message_id)
        || receipt.get("body").and_then(serde_json::Value::as_str) != Some(challenge)
        || receipt.get("subject").and_then(serde_json::Value::as_str) != Some(challenge)
        || !supported_type
    {
        anyhow::bail!("COLLAB_LIVE_CLOSURE_MESSAGE_BINDING_MISMATCH");
    }
    let message = server::state::Message {
        id: message_id.to_owned(),
        from: receipt
            .get("from")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_MESSAGE_SENDER_MISSING"))?
            .to_owned(),
        to: receipt
            .get("to")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_MESSAGE_RECIPIENT_MISSING"))?
            .to_owned(),
        mtype: receipt_type.to_owned(),
        subject: Some(challenge.to_owned()),
        body: challenge.to_owned(),
        in_reply_to: None,
        created_ms: 0,
        state: receipt
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("pending")
            .to_owned(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    let notification = server::mailbox::notification_text(&message)
        .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_NATIVE_INPUT_UNAVAILABLE"))?;
    let batch_category = live_closure_notification_category(&notification)
        .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_NATIVE_INPUT_UNAVAILABLE"))?
        .to_owned();
    let batch = server::mailbox::truncate_notification(server::mailbox::compose_notification(
        message_id,
        "notification-batch",
        &server::mailbox::batch_notification_text(
            &[(
                0,
                message_id.to_owned(),
                String::new(),
                "direct-message".to_owned(),
                notification.clone(),
            )],
            0,
        ),
    ));
    Ok(LiveClosureExpectedNativeInputs {
        exact: vec![notification, batch],
        batch_category,
    })
}

#[cfg(test)]
fn live_closure_notification_category(notification: &str) -> Option<&str> {
    notification
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(category, _)| category)
}

#[cfg(test)]
fn live_closure_batch_input_contains_message(
    observed: &str,
    message_id: &str,
    expected_category: &str,
    xml_escaped: bool,
) -> bool {
    let expected_category = if xml_escaped {
        client::adapters::codex_app_server::escape_delegated_text(expected_category)
    } else {
        expected_category.to_owned()
    };
    let Some((_, body)) = observed.split_once(" [notification-batch] Batch wake: message_ids=")
    else {
        return false;
    };
    let Some((ids, rest)) = body.split_once(" task_ids=none action_categories=") else {
        return false;
    };
    let Some((categories, _)) = rest.split_once(". Read full durable details from collab inbox;")
    else {
        return false;
    };
    ids.split(',')
        .zip(categories.split(','))
        .any(|(id, category)| id == message_id && category == expected_category)
}

#[cfg(test)]
fn live_closure_page_cursor(page: &serde_json::Value) -> anyhow::Result<Option<String>> {
    for key in ["backwardsCursor", "nextCursor"] {
        let Some(value) = page.get(key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let cursor = value
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_PAGE_CURSOR_INVALID:{key}"))?;
        if !cursor.trim().is_empty() {
            return Ok(Some(cursor.to_owned()));
        }
    }
    Ok(None)
}

#[cfg(test)]
fn read_live_closure_pages<F>(mut read_page: F) -> anyhow::Result<Vec<serde_json::Value>>
where
    F: FnMut(Option<&str>) -> anyhow::Result<serde_json::Value>,
{
    let mut cursor = None;
    let mut seen_cursors = HashSet::new();
    let mut values = Vec::new();
    loop {
        let page = read_page(cursor.as_deref())?;
        let data = page
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_ITEMS_INVALID"))?;
        values.extend(data.iter().cloned());
        let Some(next_cursor) = live_closure_page_cursor(&page)? else {
            return Ok(values);
        };
        if !seen_cursors.insert(next_cursor.clone()) {
            // Some App Server versions keep the backwards cursor anchored to
            // the same ordinal while returning the next slice. Keep the
            // current page, then stop this scan; the outer observation retry
            // remains bounded and still requires exact native correlation.
            return Ok(values);
        }
        cursor = Some(next_cursor);
    }
}

#[cfg(test)]
fn live_closure_turn_items(turns: &[serde_json::Value]) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut items = Vec::new();
    for (turn_index, turn) in turns.iter().enumerate() {
        let turn_id = turn
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_TURN_ID_MISSING:data[{turn_index}]")
            })?;
        let turn_items = turn
            .get("items")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_TURN_ITEMS_INVALID:data[{turn_index}]")
            })?;
        for (item_index, item) in turn_items.iter().enumerate() {
            let mut item = item.clone();
            let nested_payload = item.get("item");
            if nested_payload.is_some_and(|payload| !payload.is_object()) {
                anyhow::bail!(
                    "COLLAB_LIVE_CLOSURE_TARGET_ITEM_INVALID:data[{turn_index}].items[{item_index}]"
                );
            }
            for (field, value) in [
                ("turnId", item.get("turnId")),
                ("turn_id", item.get("turn_id")),
                (
                    "item.turnId",
                    nested_payload.and_then(|payload| payload.get("turnId")),
                ),
                (
                    "item.turn_id",
                    nested_payload.and_then(|payload| payload.get("turn_id")),
                ),
            ] {
                let Some(value) = value else {
                    continue;
                };
                let observed_turn_id = value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "COLLAB_LIVE_CLOSURE_TARGET_ITEM_TURN_ID_INVALID:data[{turn_index}].items[{item_index}].{field}"
                        )
                    })?;
                if observed_turn_id != turn_id {
                    anyhow::bail!(
                        "COLLAB_LIVE_CLOSURE_TARGET_ITEM_TURN_MISMATCH:data[{turn_index}].items[{item_index}]"
                    );
                }
            }
            let object = item.as_object_mut().ok_or_else(|| {
                anyhow::anyhow!(
                    "COLLAB_LIVE_CLOSURE_TARGET_ITEM_INVALID:data[{turn_index}].items[{item_index}]"
                )
            })?;
            object.insert("turnId".into(), json!(turn_id));
            items.push(item);
        }
    }
    Ok(items)
}

#[cfg(test)]
fn live_closure_turns_read_error(error: client::adapters::AdapterError) -> anyhow::Error {
    if matches!(
        &error,
        client::adapters::AdapterError::Unknown { operation: "rpc", detail }
            if detail.as_str() == "list_turns is not supported yet"
    ) {
        return anyhow::anyhow!(
            "COLLAB_LIVE_CLOSURE_TARGET_EXECUTION_PENDING:fresh_thread_materialization:{error}"
        );
    }
    anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_TURNS_READ:{error}")
}

#[cfg(test)]
fn wait_live_closure_fresh_thread_materialization<F>(
    mut observe: F,
    deadline: Instant,
    retry_delay: Duration,
) -> anyhow::Result<serde_json::Value>
where
    F: FnMut() -> anyhow::Result<serde_json::Value>,
{
    loop {
        match observe() {
            Ok(execution) => return Ok(execution),
            Err(error)
                if error.to_string().starts_with(
                    "COLLAB_LIVE_CLOSURE_TARGET_EXECUTION_PENDING:fresh_thread_materialization:",
                ) =>
            {
                if Instant::now() >= deadline {
                    anyhow::bail!(
                        "COLLAB_LIVE_CLOSURE_TARGET_EXECUTION_TIMEOUT:fresh_thread_materialization"
                    );
                }
                std::thread::sleep(retry_delay);
            }
            Err(error) => return Err(error),
        }
    }
}

fn live_closure_observe(
    scope: &Scope,
    to: String,
    challenge: String,
    message_id: String,
) -> anyhow::Result<()> {
    for (name, value) in [
        ("to", &to),
        ("challenge", &challenge),
        ("message_id", &message_id),
    ] {
        if value.trim().is_empty() {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_OBSERVE_MISSING:{name}");
        }
    }
    let ident = me(scope, None)?;
    let workers: serde_json::Value = call_project(scope, &ident, &Req::Workers)?;
    let target = workers
        .get("workers")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| {
            items.iter().find(|worker| {
                worker.get("id").and_then(serde_json::Value::as_str) == Some(to.as_str())
                    && worker
                        .get("endpoint_live")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                    && worker
                        .get("identity_valid")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
            })
        })
        .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_ROUTE_UNAVAILABLE:{to}"))?;
    let observation_deadline = Instant::now() + live_closure_timeout()?;
    let receipt = wait_live_closure_receipt(
        || {
            call_project(
                scope,
                &ident,
                &Req::MsgStatus {
                    msg_id: message_id.clone(),
                },
            )
        },
        observation_deadline,
        &message_id,
        &challenge,
    )?;
    out(&json!({
        "status": "target_receive_committed",
        "closure_claim": false,
        "target_worker_id": to,
        "receipt": receipt,
        "source": "collab recv durable consumption receipt"
    }));
    Ok(())
}

fn live_closure_daemon_producer_path(path: &str) -> bool {
    path.starts_with("daemon_") || path == "restart_replay"
}

fn live_closure_probe(
    scope: &Scope,
    closure_id: String,
    source_commit: String,
    artifact_hash: String,
    environment_id: String,
    path: String,
    to: String,
    to_project: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    const PATHS: [&str; 7] = [
        "peer_to_peer",
        "peer_to_master",
        "master_to_peer",
        "master_to_master",
        "daemon_to_peer",
        "daemon_to_master",
        "restart_replay",
    ];
    if !PATHS.contains(&path.as_str()) {
        anyhow::bail!("COLLAB_LIVE_CLOSURE_PROBE_INVALID_PATH:{path}");
    }
    for (name, value) in [
        ("closure_id", &closure_id),
        ("source_commit", &source_commit),
        ("artifact_hash", &artifact_hash),
        ("environment_id", &environment_id),
        ("to", &to),
    ] {
        if value.trim().is_empty() {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_PROBE_MISSING:{name}");
        }
    }

    let ident = me(scope, None)?;
    let context: serde_json::Value = call_project(
        scope,
        &ident,
        &Req::Context {
            worker_id: ident.worker_id.clone(),
            token: ident.token.clone(),
        },
    )?;
    let workers: serde_json::Value = call_project(scope, &ident, &Req::Workers)?;
    let master: serde_json::Value = call_project(scope, &ident, &Req::MasterStatus)?;
    let daemon_producer = live_closure_daemon_producer_path(&path);
    let endpoint_generation = ident
        .runtime
        .as_ref()
        .map(|runtime| runtime.endpoint_generation)
        .unwrap_or_default();
    let first_failure = |code: &str, detail: &str| {
        out(&json!({
            "status": "failed",
            "closure_claim": false,
            "entrypoint": "collab live-closure probe",
            "path": path,
            "first_failure": {"code": code, "detail": detail},
            "identity": ident.worker_id.clone(),
            "endpoint_generation": endpoint_generation,
        }));
    };
    if daemon_producer && to_project.is_some() {
        first_failure(
            "COLLAB_LIVE_CLOSURE_TARGET_PROJECT_UNSUPPORTED",
            "daemon-produced live-closure probes are resident-daemon contracts for the current project; cross-project daemon production requires a separate target-side contract",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_TARGET_PROJECT_UNSUPPORTED");
    }
    let target_scope = if path == "master_to_master" {
        let target = to_project
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_PROBE_MISSING:to_project"))?
            .canonicalize()?;
        if target == scope.root.canonicalize()? {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_CROSS_PROJECT_REQUIRED");
        }
        if !target.join(".agent-collab").is_dir() {
            anyhow::bail!(
                "COLLAB_LIVE_CLOSURE_TARGET_PROJECT_UNREGISTERED:{}",
                target.display()
            );
        }
        Some(Scope { root: target })
    } else {
        if to_project.is_some() {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_TARGET_PROJECT_ONLY_FOR_MASTER_TO_MASTER");
        }
        None
    };
    let target_workers: serde_json::Value = if let Some(target_scope) = target_scope.as_ref() {
        client::call_with_context(
            &target_scope.sock_path(),
            &Req::Workers,
            Some(cli_project_context(&target_scope.root)?),
        )?
    } else {
        workers.clone()
    };
    let target_master: serde_json::Value = if let Some(target_scope) = target_scope.as_ref() {
        client::call_with_context(
            &target_scope.sock_path(),
            &Req::MasterStatus,
            Some(cli_project_context(&target_scope.root)?),
        )?
    } else {
        master.clone()
    };
    if context
        .get("registered")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
        || context
            .pointer("/liveness/live")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        first_failure(
            "COLLAB_LIVE_CLOSURE_DAEMON_NOT_LIVE",
            "the probe requires an existing registered route and resident daemon",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_DAEMON_NOT_LIVE");
    }
    let target = target_workers
        .get("workers")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| {
            items.iter().find(|worker| {
                worker.get("id").and_then(serde_json::Value::as_str) == Some(to.as_str())
            })
        });
    let Some(target) = target.filter(|worker| {
        worker
            .get("endpoint_live")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            && worker
                .get("identity_valid")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
    }) else {
        first_failure(
            "COLLAB_LIVE_CLOSURE_TARGET_ROUTE_UNAVAILABLE",
            "target must already be a live authenticated worker",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_TARGET_ROUTE_UNAVAILABLE:{to}");
    };
    let master_id = master
        .pointer("/master/worker_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let target_master_id = target_master
        .pointer("/master/worker_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let sender_role = if daemon_producer {
        "daemon"
    } else if ident.worker_id == master_id {
        "master"
    } else {
        "peer"
    };
    let expected_sender_role = path.split("_to_").next().unwrap_or_default();
    if !daemon_producer && sender_role != expected_sender_role {
        first_failure(
            "COLLAB_LIVE_CLOSURE_SENDER_ROLE_MISMATCH",
            "the probe only sends as the current authenticated worker",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_SENDER_ROLE_MISMATCH");
    }
    if !daemon_producer && ident.worker_id == to {
        first_failure(
            "COLLAB_LIVE_CLOSURE_TARGET_SELF",
            "a closure path requires a distinct target worker",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_TARGET_SELF");
    }
    if path.ends_with("_to_master") && path != "master_to_master" && to != master_id {
        first_failure(
            "COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH",
            "the target is not the current live master",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH");
    }
    if matches!(path.as_str(), "daemon_to_peer" | "restart_replay")
        && !master_id.is_empty()
        && to == master_id
    {
        first_failure(
            "COLLAB_LIVE_CLOSURE_PEER_ROUTE_MISMATCH",
            "the selected daemon-produced path requires a non-master peer target",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_PEER_ROUTE_MISMATCH");
    }
    if path == "master_to_master" && to != target_master_id {
        first_failure(
            "COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH",
            "the target project route is not owned by the requested live master",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH");
    }
    let challenge = format!(
        "appsdk-collab-live:{closure_id}:{path}:{source_commit}:{artifact_hash}:{environment_id}:{endpoint_generation}"
    );
    let timeout = live_closure_timeout()?;
    let response: serde_json::Value = if let Some(target_scope) = target_scope.as_ref() {
        let assigned_by = master
            .get("master")
            .and_then(|value| value.get("assigned_by"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let assigned_ms = master
            .get("master")
            .and_then(|value| value.get("assigned_ms"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default();
        let approval = master
            .get("master")
            .and_then(|value| value.get("approval"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        client::call_with_context(
            &target_scope.sock_path(),
            &Req::CrossProjectSend {
                from: ident.worker_id.clone(),
                from_project: scope.root.display().to_string(),
                source_master_assigned_by: assigned_by.to_owned(),
                source_master_approval: approval,
                source_master_assigned_ms: assigned_ms,
                to: to.clone(),
                subject: challenge.clone(),
                body: challenge.clone(),
                in_reply_to: None,
            },
            Some(cli_project_context(&target_scope.root)?),
        )?
    } else if daemon_producer {
        call_project(
            scope,
            &ident,
            &Req::LiveClosureDaemonSend {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
                to: to.clone(),
                path: path.clone(),
                subject: challenge.clone(),
                body: challenge.clone(),
                restart_replay_pending: path == "restart_replay",
            },
        )?
    } else {
        let command = command_envelope(scope, &ident)?;
        call_project(
            scope,
            &ident,
            &Req::Send {
                from: ident.worker_id.clone(),
                worker_id: Some(ident.worker_id.clone()),
                token: Some(ident.token.clone()),
                command: Some(command),
                to: to.clone(),
                mtype: "notify".into(),
                subject: Some(challenge.clone()),
                body: challenge.clone(),
                in_reply_to: None,
                delivery: "immediate".into(),
            },
        )?
    };
    let message_id = response
        .get("message_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| response.get("msg_id").and_then(serde_json::Value::as_str))
        .or_else(|| {
            response
                .pointer("/message/id")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or_default();
    if message_id.is_empty() {
        anyhow::bail!("COLLAB_LIVE_CLOSURE_PROBE_MESSAGE_ID_MISSING");
    }
    let observation_deadline = Instant::now() + timeout;
    let receipt = wait_live_closure_receipt(
        || {
            if let Some(target_scope) = target_scope.as_ref() {
                client::call_with_context(
                    &target_scope.sock_path(),
                    &Req::MsgStatus {
                        msg_id: message_id.to_owned(),
                    },
                    Some(cli_project_context(&target_scope.root)?),
                )
            } else {
                call_project(
                    scope,
                    &ident,
                    &Req::MsgStatus {
                        msg_id: message_id.to_owned(),
                    },
                )
            }
        },
        observation_deadline,
        message_id,
        &challenge,
    )
    .map_err(|error| {
        first_failure(
            "COLLAB_LIVE_CLOSURE_RECEIPT_NOT_CONSUMED",
            &error.to_string(),
        );
        error
    })?;
    let target_master_route = target_scope.as_ref().map(|_| {
        json!({
            "worker_id": to,
            "role": "master",
            "endpoint_live": target.get("endpoint_live").and_then(serde_json::Value::as_bool),
            "identity_valid": target.get("identity_valid").and_then(serde_json::Value::as_bool),
            "transport": target.get("transport").cloned().unwrap_or_else(|| json!({})),
        })
    });
    let restart_replay_pending = response
        .get("restart_replay_pending")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let closure_claim = !restart_replay_pending;
    let status = if restart_replay_pending {
        "restart_replay_pending_observed"
    } else {
        "closure_observed"
    };
    out(&json!({
        "status": status,
        "closure_claim": closure_claim,
        "entrypoint": "collab live-closure probe",
        "path": path,
        "challenge": challenge,
        "message_id": message_id,
        "sender_worker_id": if daemon_producer {
            "collab-server".to_owned()
        } else {
            ident.worker_id.clone()
        },
        "requester_worker_id": ident.worker_id,
        "sender_role": sender_role,
        "target_worker_id": to,
        "target_project_scope": target_scope
            .as_ref()
            .map(|scope| scope.root.display().to_string()),
        "target_master_route": target_master_route,
        "message_project_scope": target_scope
            .as_ref()
            .map(|scope| scope.root.display().to_string()),
        "message_sender": if daemon_producer {
            "collab-server".to_owned()
        } else if target_scope.is_some() {
            format!("{}@{}", ident.worker_id, scope.root.display())
        } else {
            ident.worker_id.clone()
        },
        "daemon_sender": daemon_producer,
        "restart_replay_pending": restart_replay_pending,
        "restart_replay_contract": restart_replay_pending.then(|| json!({
            "status": "pending_daemon_restart",
            "reason": "the challenge was produced by the resident daemon and observed by the target, but no daemon restart/replay occurred in this probe"
        })),
        "endpoint_generation": endpoint_generation,
        "receipt": receipt,
        "source": "collab daemon route + collab recv durable consumption receipt"
    }));
    Ok(())
}

fn cli_project_context(root: &std::path::Path) -> anyhow::Result<ProjectContext> {
    ProjectContext::for_registered_root_with_app(
        root,
        AppServerId::new(identity::CLI_APP_SERVER_ID)?,
    )
}

struct ContextBootstrap {
    scope: Scope,
    project_root_resolution: &'static str,
    baseline_created: bool,
    daemon_started: bool,
}

fn resolve_context_root(
    host_paths: &scope::HostPaths,
    cwd: &Path,
) -> anyhow::Result<(Scope, &'static str)> {
    if let Ok(scope) = Scope::resolve() {
        return Ok((scope, "route"));
    }
    if let Ok(route) = scope::canonical_route_for_cwd(host_paths, cwd) {
        return Ok((Scope { root: route.root }, "canonical-route"));
    }
    let canonical_cwd = std::fs::canonicalize(cwd)?;
    let is_git_root = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&canonical_cwd)
        .output()
        .ok()
        .and_then(|output| {
            if !output.status.success() {
                return None;
            }
            std::fs::canonicalize(String::from_utf8_lossy(&output.stdout).trim()).ok()
        })
        .is_some_and(|toplevel| toplevel == canonical_cwd)
        && !canonical_cwd.ancestors().skip(1).any(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == "playground")
        });
    if canonical_cwd.join(".agent-collab").is_dir() || is_git_root {
        return Ok((
            Scope {
                root: canonical_cwd,
            },
            "cwd",
        ));
    }
    anyhow::bail!(
        "COLLAB_CONTEXT_UNRESOLVED: no registered Collab route and no local .agent-collab baseline for {}; run `collab context` from the canonical project main checkout",
        canonical_cwd.display()
    )
}

fn context_bootstrap(
    host_paths: &scope::HostPaths,
    cwd: &Path,
) -> anyhow::Result<ContextBootstrap> {
    let (scope, project_root_resolution) = resolve_context_root(host_paths, cwd)?;
    let baseline_created = if scope.root.join(".agent-collab").is_dir() {
        false
    } else {
        if scope.root.ancestors().skip(1).any(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == "playground")
        }) {
            anyhow::bail!(
                "collab context refuses to create a baseline inside a ./playground worktree"
            );
        }
        scope::init(&scope.root)?;
        true
    };
    let daemon_started = !client::alive(&scope.sock_path());
    client::ensure_server(&scope.sock_path())?;
    Ok(ContextBootstrap {
        scope,
        project_root_resolution,
        baseline_created,
        daemon_started,
    })
}

fn context_snapshot(worker: Option<String>) -> anyhow::Result<serde_json::Value> {
    let host_paths = scope::HostPaths::resolve()?;
    let cwd = std::env::current_dir()?;
    let bootstrap = context_bootstrap(&host_paths, &cwd)?;
    let scope = bootstrap.scope;
    let mut ident = identity::load_or_create(&scope, worker, None)?;
    let (_, identity_state) = ensure_registration_with_outcome(&scope, &mut ident)?;
    let identity_state = match identity_state {
        RegistrationOutcome::Created => "created",
        RegistrationOutcome::Reused => "reused",
        RegistrationOutcome::Recovered => "recovered",
        RegistrationOutcome::Recreated => "recreated",
    };
    let mut v: serde_json::Value = call_project(
        &scope,
        &ident,
        &Req::Context {
            worker_id: ident.worker_id.clone(),
            token: ident.token.clone(),
        },
    )?;
    if let Some(value) = v.as_object_mut() {
        value.insert(
            "bootstrap".to_string(),
            json!({
                "project_root_resolution": bootstrap.project_root_resolution,
                "baseline_created": bootstrap.baseline_created,
                "daemon_started": bootstrap.daemon_started,
                "identity": identity_state,
                "registered": true,
            }),
        );
    }
    Ok(v)
}

fn command_envelope(scope: &Scope, ident: &Identity) -> anyhow::Result<proto::CommandEnvelope> {
    let runtime = runtime_for_request(ident)?;
    let route = scope.route_scope(runtime.appserver_id.clone())?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(proto::CommandEnvelope::new(
        CommandId::new(format!("command-{}-{nonce}", std::process::id()))?,
        OperationId::new(format!("operation-{}-{nonce}", std::process::id()))?,
        runtime.binding_id.clone(),
        runtime.endpoint_generation,
        route,
        None,
        None,
        None,
        None,
    ))
}

/// One stable, caller-owned identity per `recv` invocation. It is generated
/// before the request so a lost response can be replayed by the same caller
/// instead of stranding an already committed batch.
fn new_receive_id() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("receive-{}-{nonce}", std::process::id())
}

/// The exact, copyable recovery instruction the default `recv` publishes
/// before it consumes anything.
fn receive_recovery_banner(receive_id: &str) -> String {
    format!("recv receive_id={receive_id} recovery: collab recv --receive-id {receive_id}")
}

pub(crate) fn recv_request(
    worker_id: String,
    token: String,
    timeout_seconds: u64,
    receive_id: String,
) -> Req {
    Req::Poll {
        worker_id,
        token,
        timeout_ms: timeout_seconds.saturating_mul(1000),
        receive_id: Some(receive_id),
    }
}

/// Publish the caller-owned receive identity and then dispatch the request.
///
/// Claiming `receive_id` only after a successful reply cannot recover a route
/// whose response was lost, so the default CLI must make the replay identity
/// visible before the daemon consumes the batch. A failed publication returns
/// before dispatch: consuming without a recoverable identity would recreate
/// the stranded batch the identity exists to prevent.
fn recv_with_published_identity<D>(
    publish: &mut dyn std::io::Write,
    receive_id: &str,
    dispatch: D,
) -> anyhow::Result<serde_json::Value>
where
    D: FnOnce(&str) -> anyhow::Result<serde_json::Value>,
{
    writeln!(publish, "{}", receive_recovery_banner(receive_id))
        .and_then(|()| publish.flush())
        .map_err(|error| anyhow::anyhow!("RECEIVE_IDENTITY_PUBLISH_FAILED: {error}"))?;
    dispatch(receive_id)
}

fn subagent_observe_query(command: &subagent::Action) -> Option<(Option<String>, Option<usize>)> {
    match command {
        subagent::Action::List => Some((None, None)),
        subagent::Action::Status { id } => Some((Some(id.clone()), None)),
        _ => None,
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli.cmd) {
        eprintln!("collab: {}", format_cli_error(&e.to_string()));
        std::process::exit(1);
    }
}

const LEGACY_ROUTE_RESOLVE_NOT_FOUND_RECOVERY: &str = "recovery: run `collab context` from the canonical project main checkout to resolve the route and restore registration; do not re-register a worktree or edit routes.jsonl";
const LEGACY_ROUTE_RESOLVE_NOT_FOUND_UPGRADE: &str = "recovery: run `collab context` from the canonical project main checkout; preserve daemon state and do not start a second daemon or use mailbox state as transport delivery";
const IDENTITY_REBIND_UNPROVEN_RECOVERY: &str = "recovery: run `collab context` from the canonical project main checkout; if the same identity error persists, preserve the exact error and worker_id and report it to the live master; do not edit routes, copy tokens, or start a second daemon";
const IDENTITY_RESTORE_CROSS_PROJECT_RECOVERY: &str = "recovery: run `collab context` from the canonical project main checkout for the current project; preserve the exact error and report it to the live master if it persists; do not edit routes, copy tokens, or start a second daemon";

fn format_cli_error(error: &str) -> String {
    let decorated = if error.starts_with("ROUTE_RESOLVE_NOT_FOUND:")
        && !error.contains(crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY)
        && !error.contains(LEGACY_ROUTE_RESOLVE_NOT_FOUND_UPGRADE)
    {
        if error.contains(LEGACY_ROUTE_RESOLVE_NOT_FOUND_RECOVERY) {
            format!("{error}; {LEGACY_ROUTE_RESOLVE_NOT_FOUND_UPGRADE}")
        } else {
            format!(
                "{error}; {}",
                crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY
            )
        }
    } else {
        error.to_owned()
    };
    if decorated.starts_with("IDENTITY_REBIND_UNPROVEN:")
        && !decorated.contains(IDENTITY_REBIND_UNPROVEN_RECOVERY)
    {
        return format!("{decorated}; {IDENTITY_REBIND_UNPROVEN_RECOVERY}");
    }
    if decorated.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:")
        && !decorated.contains(IDENTITY_RESTORE_CROSS_PROJECT_RECOVERY)
    {
        return format!("{decorated}; {IDENTITY_RESTORE_CROSS_PROJECT_RECOVERY}");
    }
    decorated
}

fn run(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Config => {
            crate::config::ensure_written()?;
            out(&crate::config::load(&scope::lifecycle_project_root()?)?);
            Ok(())
        }
        Cmd::Subagent { command } => {
            if matches!(&command, subagent::Action::Start { .. }) {
                anyhow::bail!("MANAGED_SUBAGENT_UNSUPPORTED: tmux cannot create a Codex thread; start the peer in its own tmux pane and register that pane");
            }
            let scope = Scope::resolve()?;
            let query = subagent_observe_query(&command);
            if let Some((id, snapshot_lines)) = query {
                let value: serde_json::Value = client::call_with_context(
                    &scope.sock_path(),
                    &Req::SubagentObserve { id, snapshot_lines },
                    Some(cli_project_context(&scope.root)?),
                )?;
                out(&value);
                return Ok(());
            }
            let ident = me(&scope, None)?;
            let value: serde_json::Value = call_project(
                &scope,
                &ident,
                &Req::Subagent {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    command,
                    launch_env: Default::default(),
                },
            )?;
            out(&value);
            Ok(())
        }
        Cmd::Init { worker_id } => {
            let project_root = scope::project_root_for_init()?;
            if project_root.ancestors().skip(1).any(|ancestor| {
                ancestor
                    .file_name()
                    .is_some_and(|name| name == "playground")
            }) {
                anyhow::bail!(
                    "collab init must run from the project main tree, not a ./playground worktree"
                );
            }
            let _base = scope::init(&project_root)?;
            let scope = Scope { root: project_root };
            let started = !client::alive(&scope.sock_path());
            client::ensure_server(&scope.sock_path())?;
            let mut ident = identity::load_or_create_for_init(&scope, worker_id)?;
            let registration = ensure_registration(&scope, &mut ident)?;
            let daemon_pid = std::fs::read_to_string(scope.host_paths()?.pid_path())
                .map_err(|error| anyhow::anyhow!("registered daemon PID is unavailable: {error}"))?
                .trim()
                .parse::<u32>()
                .map_err(|error| anyhow::anyhow!("registered daemon PID is invalid: {error}"))?;
            let runtime = registered_runtime_projection(&scope, &ident, daemon_pid)?;
            let task_board: serde_json::Value =
                call_project(&scope, &ident, &Req::TaskStatus { task_id: None })?;
            out(&json!({
                "ok": true,
                "root": scope.root,
                "worker_id": ident.worker_id,
                "identity_kind": "peer",
                "runtime": runtime,
                "transport_selected": ident.transport,
                "daemon_started": started,
                "role_brief": registration["role_brief"],
                "task_board": task_board["tasks"],
                "recovery_action": registration["role_brief"]["communication_recovery"]
            }));
            Ok(())
        }
        Cmd::Serve => {
            let scope = Scope {
                root: scope::lifecycle_project_root()?,
            };
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(server::run(scope))
        }
        Cmd::Up => {
            let project_root = scope::lifecycle_project_root()?;
            if !project_root.join(".agent-collab").is_dir() {
                scope::init(&project_root)?;
            }
            let scope = Scope { root: project_root };
            let host_server_dir = scope.host_server_dir();
            std::fs::create_dir_all(&host_server_dir)?;
            std::fs::remove_file(host_server_dir.join("DOWN")).ok();
            client::record_event(
                &scope.sock_path(),
                "daemon_up_requested",
                json!({"pid": std::process::id()}),
            );
            let sock = scope.sock_path();
            let was_running = client::alive(&sock);
            client::ensure_server(&sock)?;
            out(&json!({"ok": true, "server": sock, "started": !was_running}));
            Ok(())
        }
        Cmd::Down => {
            let scope = Scope {
                root: scope::lifecycle_project_root()?,
            };
            if client::alive(&scope.sock_path()) {
                let _: serde_json::Value = client::call_with_context(
                    &scope.sock_path(),
                    &Req::Shutdown { operator: true },
                    Some(cli_project_context(&scope.root)?),
                )?;
            }
            let server_dir = scope.host_server_dir();
            std::fs::create_dir_all(&server_dir)?;
            std::fs::write(server_dir.join("DOWN"), b"explicitly stopped\n")?;
            client::record_event(
                &scope.sock_path(),
                "daemon_down_requested",
                json!({"pid": std::process::id()}),
            );
            let pid_path = server_dir.join("server.pid");
            if client::alive(&scope.sock_path()) {
                let mut pids = Vec::new();
                let output = std::process::Command::new("lsof")
                    .args(["-t", scope.sock_path().to_str().unwrap_or_default()])
                    .output()?;
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    if let Ok(pid) = line.trim().parse::<i32>() {
                        pids.push(pid);
                    }
                }
                if pids.is_empty() {
                    if let Ok(pid_text) = std::fs::read_to_string(&pid_path) {
                        if let Ok(pid) = pid_text.trim().parse::<i32>() {
                            pids.push(pid);
                        }
                    }
                }
                pids.sort_unstable();
                pids.dedup();
                for pid in pids {
                    if pid > 1 && pid != std::process::id() as i32 {
                        let status = std::process::Command::new("kill")
                            .args(["-TERM", &pid.to_string()])
                            .status()?;
                        if !status.success() {
                            anyhow::bail!("failed to stop collab daemon pid {}", pid);
                        }
                    }
                }
                for _ in 0..40 {
                    if !client::alive(&scope.sock_path()) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                if client::alive(&scope.sock_path()) {
                    anyhow::bail!(
                        "collab daemon did not stop at {}",
                        scope.sock_path().display()
                    );
                }
            }
            out(&json!({"ok": true, "down": true, "server": scope.sock_path()}));
            Ok(())
        }
        Cmd::Status { all } => {
            let scope = Scope::resolve()?;
            let v: serde_json::Value = if all {
                client::call_with_context(
                    &scope.sock_path(),
                    &Req::StatusAll,
                    Some(cli_project_context(&scope.root)?),
                )?
            } else {
                client::call(&scope.sock_path(), &Req::Ping)?
            };
            out(&v);
            Ok(())
        }
        Cmd::Mailbox { cmd } => {
            let scope = Scope::resolve()?;
            match cmd {
                MailboxCmd::Read { all, sort, worker } => {
                    let explicit_worker = worker.is_some();
                    let actorless = all || explicit_worker;
                    let mut identity = None;
                    let worker_id = if actorless {
                        worker
                    } else {
                        let ident = me(&scope, None)?;
                        let worker_id = ident.worker_id.clone();
                        identity = Some(ident);
                        Some(worker_id)
                    };
                    let request = Req::MailboxRead {
                        all,
                        sort: Some(sort),
                        worker_id,
                    };
                    let v: serde_json::Value = if actorless {
                        client::call_with_context(
                            &scope.sock_path(),
                            &request,
                            Some(cli_project_context(&scope.root)?),
                        )?
                    } else {
                        let ident = identity
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("mailbox identity was not resolved"))?;
                        call_project(&scope, ident, &request)?
                    };
                    out(&v);
                    Ok(())
                }
            }
        }
        Cmd::Role => {
            anyhow::bail!("collab role is deprecated; declared roles were removed")
        }
        Cmd::Who => {
            let scope = Scope::resolve()?;
            let v: serde_json::Value = client::call_with_context(
                &scope.sock_path(),
                &Req::Workers,
                Some(cli_project_context(&scope.root)?),
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Route {
            command:
                RouteCmd::Resolve {
                    session_id,
                    pane_id,
                },
        } => {
            let host_paths = scope::HostPaths::resolve()?;
            let candidate =
                crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?;
            if session_id
                .as_deref()
                .is_some_and(|value| value != candidate.endpoint.tmux_session_id)
                || pane_id
                    .as_deref()
                    .is_some_and(|value| value != candidate.endpoint.pane_id)
            {
                anyhow::bail!(
                    "ROUTE_RESOLVE_INVALID: requested identity does not match current tmux pane"
                );
            }
            let route =
                crate::client::resolve_route(&host_paths.socket_path(), &candidate.endpoint)?;
            out(&json!({
                "canonical_root": route.canonical_root,
                "storage_root": route.storage_root,
                "app_scope_id": route.app_scope_id,
                "project_scope": route.project_scope,
                "agent_id": route.agent_id,
                "binding_id": route.binding_id,
                "endpoint_generation": route.endpoint_generation,
                "session_id": route.session_id,
                "native_thread_id": route.native_thread_id,
                "tmux_endpoint": candidate.endpoint,
            }));
            Ok(())
        }
        Cmd::LiveClosure {
            command:
                LiveClosureCmd::Probe {
                    closure_id,
                    source_commit,
                    artifact_hash,
                    environment_id,
                    path,
                    to,
                    to_project,
                },
        } => {
            let scope = Scope::resolve()?;
            live_closure_probe(
                &scope,
                closure_id,
                source_commit,
                artifact_hash,
                environment_id,
                path,
                to,
                to_project,
            )
        }
        Cmd::LiveClosure {
            command:
                LiveClosureCmd::Observe {
                    to,
                    challenge,
                    message_id,
                },
        } => {
            let scope = Scope::resolve()?;
            live_closure_observe(&scope, to, challenge, message_id)
        }
        Cmd::Root { command } | Cmd::Master { command } => {
            if matches!(command, MasterCmd::Status) {
                let host_paths = scope::HostPaths::resolve()?;
                let route = scope::route_for_tmux_pane(&host_paths)?;
                let scope = Scope { root: route.root };
                let v: serde_json::Value = client::call_with_context(
                    &scope.sock_path(),
                    &Req::MasterStatus,
                    Some(proto::ProjectContext::for_registered_root_with_app(
                        &scope.root,
                        route.app_scope_id,
                    )?),
                )?;
                out(&v);
                return Ok(());
            }
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let req = match command {
                MasterCmd::Recover => {
                    anyhow::bail!(
                        "collab master recover is deprecated; use collab master promote or delegate"
                    )
                }
                MasterCmd::Promote { approval } => Req::MasterPromote {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    approval,
                },
                MasterCmd::Delegate { target } => Req::MasterDelegate {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    target_id: target,
                },
                MasterCmd::Send {
                    project,
                    to,
                    subject,
                    body,
                } => {
                    let local: serde_json::Value =
                        call_project(&scope, &ident, &Req::MasterStatus)?;
                    let Some(master) = local.get("master") else {
                        anyhow::bail!("cross-project send requires this peer to be a live master")
                    };
                    if master.get("worker_id").and_then(|v| v.as_str())
                        != Some(ident.worker_id.as_str())
                        || master.get("endpoint_live").and_then(|v| v.as_bool()) != Some(true)
                    {
                        anyhow::bail!("cross-project send requires this peer to be the live master")
                    }
                    let target = project.canonicalize()?;
                    if target == scope.root.canonicalize()? {
                        anyhow::bail!("cross-project send requires a different project")
                    }
                    if !target.join(".agent-collab").is_dir() {
                        anyhow::bail!("target project has no .agent-collab: {}", target.display())
                    }
                    let target_scope = Scope { root: target };
                    let value: serde_json::Value = client::call_with_context(
                        &target_scope.sock_path(),
                        &Req::CrossProjectSend {
                            from: ident.worker_id.clone(),
                            from_project: scope.root.display().to_string(),
                            source_master_assigned_by: master
                                .get("assigned_by")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            source_master_approval: master
                                .get("approval")
                                .and_then(|v| v.as_str())
                                .map(str::to_owned),
                            source_master_assigned_ms: master
                                .get("assigned_ms")
                                .and_then(|v| v.as_i64())
                                .unwrap_or_default(),
                            to,
                            subject,
                            body: body.join(" "),
                            in_reply_to: None,
                        },
                        Some(cli_project_context(&target_scope.root)?),
                    )?;
                    out(&value);
                    return Ok(());
                }
                MasterCmd::Status => unreachable!("handled above"),
            };
            let v: serde_json::Value = call_project(&scope, &ident, &req)?;
            out(&v);
            Ok(())
        }
        Cmd::Worker { cmd } => {
            let scope = match cmd {
                WorkerCmd::Recover => scope::resolve_for_recovery()?,
                _ => Scope::resolve()?,
            };
            match cmd {
                WorkerCmd::Recover => {
                    let mut ident = identity::load_or_create(&scope, None, None)?;
                    let _ = register_recovery(&scope, &mut ident)?;
                    out(&json!({
                        "recovered": true,
                        "worker_id": ident.worker_id,
                        "transport": ident.transport,
                        "identity_kind": "peer",
                        "next": "run collab who and collab task status; task ownership is unchanged"
                    }));
                    Ok(())
                }
                WorkerCmd::Status { id } => {
                    let ident = me(&scope, None)?;
                    let v: serde_json::Value =
                        call_project(&scope, &ident, &Req::WorkerStatus { worker_id: id })?;
                    out(&v);
                    Ok(())
                }
                WorkerCmd::Snapshot { id, lines } => {
                    let ident = me(&scope, None)?;
                    let v: serde_json::Value = call_project(
                        &scope,
                        &ident,
                        &Req::WorkerSnapshot {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                            target_id: id,
                            lines,
                        },
                    )?;
                    out(&v);
                    Ok(())
                }
                WorkerCmd::Close { id, reason } => {
                    let ident = me(&scope, None)?;
                    let v: serde_json::Value = call_project(
                        &scope,
                        &ident,
                        &Req::WorkerClose {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                            target_id: id,
                            reason,
                        },
                    )?;
                    out(&v);
                    Ok(())
                }
            }
        }
        Cmd::TransferMaster { target } => {
            let _ = target;
            anyhow::bail!("collab transfer-master is deprecated; use collab master delegate")
        }
        Cmd::RemoveWorker { target, force } => {
            let _ = (target, force);
            anyhow::bail!(
                "collab remove-worker is deprecated; use owner cleanup and migration verify"
            )
        }
        Cmd::Reset {
            approval,
            discard_legacy,
        } => {
            let root = scope::project_root_for_init()?;
            let scope = Scope { root };
            let host_paths = scope::HostPaths::resolve()?;
            reset::run(
                &scope,
                &host_paths,
                reset::ResetRequest {
                    approval: approval.unwrap_or_default(),
                    discard_legacy,
                },
            )
        }
        Cmd::Whoami { worker } => {
            let scope = Scope::resolve()?;
            let mut ident = identity::load_or_create(&scope, worker, None)?;
            let registration = ensure_registration(&scope, &mut ident)?;
            let mut response = serde_json::to_value(&ident)?;
            response["role_brief"] = registration["role_brief"].clone();
            out(&response);
            Ok(())
        }
        Cmd::Send {
            from,
            to,
            subject,
            r#type,
            in_reply_to,
            delivery,
            body,
        } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            if from
                .as_deref()
                .is_some_and(|requested| requested != ident.worker_id)
            {
                anyhow::bail!("--from must match the authenticated worker identity");
            }
            let body = body.join(" ");
            if body.is_empty() {
                anyhow::bail!("empty message body");
            }
            let command = command_envelope(&scope, &ident)?;
            let v: serde_json::Value = call_project(
                &scope,
                &ident,
                &Req::Send {
                    from: ident.worker_id.clone(),
                    worker_id: Some(ident.worker_id.clone()),
                    token: Some(ident.token.clone()),
                    command: Some(command),
                    to,
                    mtype: r#type,
                    subject: Some(subject),
                    body,
                    in_reply_to,
                    delivery,
                },
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Notify { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let request = match cmd {
                NotifyCmd::Methods => Req::NotificationMethods,
                NotifyCmd::Subscribe {
                    event,
                    subject,
                    at_ms,
                    every_ms,
                    repeat_count,
                    trigger_ms,
                    ttl_seconds,
                } => Req::NotificationSubscribe {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    event,
                    subject,
                    trigger_ms,
                    trigger_times_ms: at_ms,
                    interval_ms: every_ms,
                    repeat_count,
                    ttl_seconds,
                },
                NotifyCmd::Status => Req::NotificationStatus {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                },
                NotifyCmd::Unsubscribe { subscription_id } => Req::NotificationUnsubscribe {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    subscription_id,
                },
            };
            let value: serde_json::Value = call_project(&scope, &ident, &request)?;
            out(&value);
            Ok(())
        }
        Cmd::Recv {
            timeout,
            worker,
            receive_id,
        } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, worker)?;
            let receive_id = receive_id.unwrap_or_else(new_receive_id);
            // The caller must know the receive identity before consumption, not
            // only after a successful reply: a lost socket response still
            // leaves the exact replay command available on stderr.
            let v =
                recv_with_published_identity(&mut std::io::stderr(), &receive_id, |receive_id| {
                    call_project(
                        &scope,
                        &ident,
                        &recv_request(
                            ident.worker_id.clone(),
                            ident.token.clone(),
                            timeout,
                            receive_id.to_owned(),
                        ),
                    )
                })?;
            out(&v);
            Ok(())
        }
        Cmd::Inbox { worker } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, worker)?;
            let v: serde_json::Value = call_project(
                &scope,
                &ident,
                &Req::Inbox {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                },
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Context { worker } => {
            out(&context_snapshot(worker)?);
            Ok(())
        }
        Cmd::Ack { ids, worker, all } => {
            if ids.is_empty() && !all {
                anyhow::bail!("usage: collab ack <msg_id>... [--all] [--worker <id>]");
            }
            let scope = Scope::resolve()?;
            let ident = me(&scope, worker)?;
            let v: serde_json::Value = call_project(
                &scope,
                &ident,
                &Req::Ack {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    ids,
                },
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Msg { msg_id } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let v: serde_json::Value = call_project(&scope, &ident, &Req::MsgStatus { msg_id })?;
            out(&v);
            Ok(())
        }
        Cmd::Task { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let worker_id = ident.worker_id.clone();
            let token = ident.token.clone();
            let req = match cmd {
                TaskCmd::Register {
                    id,
                    owner,
                    feature,
                    worktree,
                    branch,
                    base_commit,
                    priority,
                    next,
                    goal,
                } => Req::TaskRegister {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    owner,
                    feature_id: feature,
                    worktree_path: worktree,
                    branch,
                    base_commit,
                    priority: priority.unwrap_or_else(crate::server::state::default_priority),
                    next_step: next,
                    goal_prompt: goal,
                },
                TaskCmd::Update { id, status, next } => Req::TaskUpdate {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    status,
                    next_step: next,
                },
                TaskCmd::Accept { id } => Req::TaskAccept {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                },
                TaskCmd::Relocate {
                    id,
                    worktree,
                    branch,
                    base_commit,
                } => Req::TaskRelocate {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    worktree_path: worktree,
                    branch,
                    base_commit,
                },
                TaskCmd::Claim { id } => Req::TaskClaim {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                },
                TaskCmd::Wait { id, blocking_task } => Req::TaskWait {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    blocking_task_id: blocking_task,
                },
                TaskCmd::Deliver {
                    id,
                    evidence,
                    worktree,
                } => Req::TaskDeliver {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    evidence: Some(evidence),
                    worktree: Some(worktree),
                },
                TaskCmd::Review {
                    id,
                    accept,
                    rework,
                    evidence,
                } => Req::TaskReview {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    accept,
                    rework,
                    evidence,
                },
                TaskCmd::Integrated {
                    id,
                    commit,
                    evidence,
                } => Req::TaskIntegrated {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    commit,
                    evidence,
                },
                TaskCmd::Block { id, next } => Req::TaskUpdate {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    status: Some("blocked".into()),
                    next_step: next,
                },
                TaskCmd::Close { id, force, reason } => Req::TaskClose {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    force,
                    reason,
                },
                TaskCmd::FinalizeCleanup { id } => Req::TaskFinalizeCleanup {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                },
                TaskCmd::Dispatch => Req::TaskDispatch {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
                TaskCmd::Status { id } => Req::TaskStatus { task_id: id },
            };
            let v: serde_json::Value = call_project(&scope, &ident, &req)?;
            out(&v);
            Ok(())
        }
        Cmd::Migrate { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let worker_id = ident.worker_id.clone();
            let token = ident.token.clone();
            let req = match cmd {
                MigrateCmd::Inspect => Req::MigrationInspect {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
                MigrateCmd::Plan => Req::MigrationPlan {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
                MigrateCmd::Apply => Req::MigrationApply {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
                MigrateCmd::Verify => Req::MigrationVerify {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
            };
            let v: serde_json::Value = call_project(&scope, &ident, &req)?;
            out(&v);
            Ok(())
        }
        Cmd::InstallSkills { target, force } => {
            let target = match target {
                Some(path) => path,
                None => match std::env::var_os("HOME") {
                    Some(home) => std::path::PathBuf::from(home)
                        .join(".agents")
                        .join("skills")
                        .join("collab"),
                    None => {
                        anyhow::bail!("install-skills default target requires $HOME; pass --target")
                    }
                },
            };
            let (outcomes, bytes, count) =
                install_skills::install(&target, force).map_err(|error| anyhow::anyhow!(error))?;
            let written = outcomes
                .iter()
                .filter(|(_, o)| *o == install_skills::InstallOutcome::Written)
                .count();
            let skipped = count - written;
            let files: Vec<&str> = outcomes.iter().map(|(r, _)| *r).collect();
            out(&serde_json::json!({
                "target": target,
                "files": files,
                "written": written,
                "skipped": skipped,
                "bytes": bytes,
                "force": force,
                "next": "the collab skill is now visible to any agent that loads ~/.agents/skills; restart the agent or rerun its skill discovery to pick up the bundle",
            }));
            Ok(())
        }
    }
}

// keep Resp referenced so the type stays part of the public surface for tests
#[allow(dead_code)]
fn _unused(_r: Resp) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{SelectedTransport, TransportKind};
    use std::io::BufRead;

    fn test_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cm-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn identity_with_runtime(runtime: Option<RuntimeIdentity>) -> Identity {
        Identity {
            worker_id: "worker-1".into(),
            token: "token-1".into(),
            project_scope: None,
            runtime,
            transport: None,
        }
    }

    #[test]
    fn init_accepts_explicit_worker_id_for_pane_free_registration() {
        let cli = Cli::try_parse_from([
            "collab",
            "init",
            "--worker-id",
            "codex-thread-6465736b746f702d746872656164",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Init { worker_id: Some(worker_id) }
                if worker_id == "codex-thread-6465736b746f702d746872656164"
        ));
    }

    #[test]
    fn cli_rejects_managed_subagent_start_before_resolving_collab_context() {
        let error = run(Cmd::Subagent {
            command: subagent::Action::Start {
                id: Some("child-test".into()),
                runtime: None,
            },
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("MANAGED_SUBAGENT_UNSUPPORTED:"),
            "unexpected start result: {error:#}"
        );
    }

    /// Bind the process environment to one explicit App Server address while
    /// holding the shared env lock.  Registration reuse now compares the
    /// persisted thread/session against the live ones, so a test that builds
    /// a thread-backed fixture must state the address it means instead of
    /// inheriting whatever the developer's shell exported.
    fn set_current_session_thread(thread_id: &str, session_id: &str) {
        std::env::set_var("CODEX_THREAD_ID", thread_id);
        std::env::set_var("CODEX_SESSION_ID", session_id);
    }

    fn clear_current_session_thread() {
        std::env::remove_var("CODEX_THREAD_ID");
        std::env::remove_var("CODEX_SESSION_ID");
    }

    #[test]
    fn runtime_for_request_rejects_an_unregistered_identity() {
        let identity = identity_with_runtime(None);
        let error = runtime_for_request(&identity).unwrap_err();
        assert!(error
            .to_string()
            .contains("identity has no registered runtime binding"));
    }

    #[test]
    fn runtime_for_request_accepts_a_persisted_tui_binding() {
        let runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("worker-1").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-tui").unwrap(),
            appserver_id: identity::AppServerId::new("tui-default").unwrap(),
            endpoint_generation: 3,
            binding_id: identity::BindingId::new("binding-tui").unwrap(),
            session_id: None,
            native_thread_id: None,
        };
        let identity = identity_with_runtime(Some(runtime.clone()));
        assert_eq!(runtime_for_request(&identity).unwrap(), &runtime);
    }

    #[test]
    fn live_closure_item_turn_id_is_required_for_native_correlation() {
        assert_eq!(
            live_closure_item_turn_id(&json!({"text": "challenge message"})),
            None
        );
        assert_eq!(
            live_closure_item_turn_id(&json!({"turnId": "turn-1"})),
            Some("turn-1")
        );
        assert_eq!(
            live_closure_item_turn_id(&json!({"turn_id": "turn-2"})),
            Some("turn-2")
        );
        assert_eq!(live_closure_item_turn_id(&json!({"turnId": "  "})), None);
    }

    #[test]
    fn live_closure_item_message_id_accepts_native_notification_client_id() {
        assert_eq!(
            live_closure_item_message_id(&json!({
                "clientId": "collab-notification-message-1"
            })),
            Some("message-1")
        );
        assert_eq!(
            live_closure_item_message_id(&json!({
                "clientUserMessageId": "message-2"
            })),
            Some("message-2")
        );
        assert_eq!(live_closure_item_message_id(&json!({})), None);
    }

    #[test]
    fn live_closure_expected_inputs_match_single_and_batch_notification_payloads() {
        let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
        let expected = live_closure_expected_native_inputs(
            &json!({
                "id": "message-1",
                "from": "sender",
                "to": "recipient",
                "type": "notify",
                "subject": challenge,
                "body": challenge,
                "state": "pending"
            }),
            "message-1",
            challenge,
        )
        .unwrap();

        assert_eq!(expected.exact.len(), 2);
        assert!(expected.exact[0].starts_with("COLLAB_NOTIFY message-1 ["));
        assert!(expected.exact[0].contains(challenge));
        assert!(expected.exact[0].contains("READ IS NOT DONE"));
        assert!(expected.exact[1].starts_with("COLLAB_NOTIFY message-1 [notification-batch]"));
        assert!(expected.exact[1].contains("message_ids=message-1"));
        assert!(expected.exact[1].contains("READ IS NOT DONE"));
        assert!(live_closure_item_matches_input(
            &json!({
                "turnId": "turn-1",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": format!(
                    "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-1</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
                    client::adapters::codex_app_server::escape_delegated_text(&expected.exact[0])
                )
            }),
            &expected,
            "message-1"
        ));
        assert!(live_closure_item_matches_input(
            &json!({
                "turnId": "turn-1",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": format!(
                    "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-1</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
                    client::adapters::codex_app_server::escape_delegated_text(&expected.exact[1])
                )
            }),
            &expected,
            "message-1"
        ));
    }

    #[test]
    fn live_closure_expected_inputs_accept_daemon_notification_message_type() {
        let challenge = "appsdk-collab-live:closure-1:daemon_to_peer";
        let expected = live_closure_expected_native_inputs(
            &json!({
                "id": "message-daemon",
                "from": "collab-server",
                "to": "recipient",
                "type": "notification",
                "subject": challenge,
                "body": challenge,
                "state": "pending"
            }),
            "message-daemon",
            challenge,
        )
        .unwrap();

        assert_eq!(expected.exact.len(), 2);
        assert!(expected.exact[0].starts_with("COLLAB_NOTIFY message-daemon ["));
        assert!(expected.exact[0].contains(challenge));
        assert!(expected.exact[1].contains("message_ids=message-daemon"));
    }

    #[test]
    fn live_closure_daemon_producer_paths_are_current_project_contracts() {
        assert!(live_closure_daemon_producer_path("daemon_to_peer"));
        assert!(live_closure_daemon_producer_path("daemon_to_master"));
        assert!(live_closure_daemon_producer_path("restart_replay"));
        assert!(!live_closure_daemon_producer_path("master_to_master"));
        assert!(!live_closure_daemon_producer_path("peer_to_peer"));
    }

    #[test]
    fn live_closure_expected_input_rejects_raw_challenge_as_native_payload() {
        let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
        let expected = live_closure_expected_native_inputs(
            &json!({
                "id": "message-1",
                "from": "sender",
                "to": "recipient",
                "type": "notify",
                "subject": challenge,
                "body": challenge,
                "state": "pending"
            }),
            "message-1",
            challenge,
        )
        .unwrap();
        assert!(!live_closure_item_matches_input(
            &json!({
                "turnId": "turn-1",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": format!(
                    "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-1</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
                )
            }),
            &expected,
            "message-1"
        ));
    }

    #[test]
    fn live_closure_item_correlation_accepts_multi_message_batch_entry() {
        let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
        let expected = live_closure_expected_native_inputs(
            &json!({
                "id": "message-target",
                "from": "sender",
                "to": "recipient",
                "type": "notify",
                "subject": challenge,
                "body": challenge,
                "state": "pending"
            }),
            "message-target",
            challenge,
        )
        .unwrap();
        let batch_input = "COLLAB_NOTIFY message-other [notification-batch] Batch wake: message_ids=message-other,message-target,message-later task_ids=none action_categories=other,appsdk-collab-live:closure-1:peer_to_peer,later. Read full durable details from collab inbox; execute the actions, do not ACK-only. older_messages=2; run collab inbox | P1 ACTION: do the in-scope action the message asks for. Details: collab msg message-other. | READ IS NOT DONE: never end your turn on an ACK, a read, or a summary. After handling, resume your current task; if you own none, run `appsdk longhorizon show` and take work.";

        assert!(live_closure_item_matches_input(
            &json!({
                "turnId": "turn-target",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": format!(
                    "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-other</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
                    client::adapters::codex_app_server::escape_delegated_text(batch_input)
                )
            }),
            &expected,
            "message-target"
        ));
        assert!(live_closure_item_matches_input(
            &json!({
                "turnId": "turn-target",
                "type": "userMessage",
                "clientId": "collab-notification-message-target",
                "content": [{"type": "text", "text": batch_input}]
            }),
            &expected,
            "message-target"
        ));

        let mismatched = batch_input.replace(
            "other,appsdk-collab-live:closure-1:peer_to_peer,later",
            "other,appsdk-collab-live:other,later",
        );
        assert!(!live_closure_item_matches_input(
            &json!({
                "turnId": "turn-target",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": format!(
                    "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-other</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
                    client::adapters::codex_app_server::escape_delegated_text(&mismatched)
                )
            }),
            &expected,
            "message-target"
        ));
    }

    #[test]
    fn live_closure_item_correlation_accepts_native_item_envelopes() {
        let item = json!({
            "turnId": "turn-envelope",
            "item": {
                "type": "userMessage",
                "id": "item-envelope",
                "clientId": "collab-notification-message-envelope"
            }
        });
        assert_eq!(live_closure_item_turn_id(&item), Some("turn-envelope"));
        assert_eq!(
            live_closure_item_message_id(&item),
            Some("message-envelope")
        );
        assert_eq!(live_closure_item_payload(&item)["id"], "item-envelope");
    }

    #[test]
    fn live_closure_item_correlation_accepts_send_message_function_call_output() {
        let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
        let item = json!({
            "type": "functionCallOutput",
            "id": "fco_01a0bee4-83d0-7f40-9e5e-2f8d9b9c564f",
            "name": "send_message_to_thread",
            "namespace": "codex_tui",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>01a0b92e-bc55-75e1-8078-8c55e59cfd1d</source_thread_id>\n  <client_message_id>collab-notification-message-target</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
            )
        });
        let turns = vec![json!({
            "id": "turn-target",
            "status": "completed",
            "items": [item, {
                "type": "agentMessage",
                "id": "result-target",
                "text": "Target peer remains ready."
            }]
        })];

        let items = live_closure_turn_items(&turns).unwrap();
        let expected = LiveClosureExpectedNativeInputs {
            exact: vec![challenge.to_owned()],
            batch_category: "closure".to_owned(),
        };
        assert!(live_closure_item_matches_input(
            &items[0],
            &expected,
            "message-target"
        ));
    }

    #[test]
    fn live_closure_item_correlation_accepts_daemon_function_output_without_source_thread() {
        let challenge = "appsdk-collab-live:closure-1:daemon_to_peer";
        let item = json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "id": "fco_01a0c851-6d67-7872-abca-7266849ef9a8",
            "name": "send_message_to_thread",
            "namespace": "codex_tui",
            "output": format!(
                "<codex_delegation>\n  <client_message_id>collab-notification-message-target</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
            )
        });

        let expected = LiveClosureExpectedNativeInputs {
            exact: vec![challenge.to_owned()],
            batch_category: "closure".to_owned(),
        };
        assert!(live_closure_item_matches_input(
            &item,
            &expected,
            "message-target"
        ));
    }

    #[test]
    fn live_closure_item_correlation_rejects_mismatched_prefixed_function_call_output() {
        let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
        let item = json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>collab-notification-message-other</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
            )
        });

        let expected = LiveClosureExpectedNativeInputs {
            exact: vec![challenge.to_owned()],
            batch_category: "closure".to_owned(),
        };
        assert!(!live_closure_item_matches_input(
            &item,
            &expected,
            "message-target"
        ));
    }

    #[test]
    fn live_closure_item_correlation_accepts_xml_escaped_function_call_output() {
        let challenge = "appsdk-collab-live:a & b < c > d";
        let item = json!({
            "turnId": "turn-target",
            "type": "functionCallOutput",
            "name": "send_message_to_thread",
            "output": format!(
                "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>message-target</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
                client::adapters::codex_app_server::escape_delegated_text(challenge)
            )
        });

        let expected = LiveClosureExpectedNativeInputs {
            exact: vec![challenge.to_owned()],
            batch_category: "closure".to_owned(),
        };
        assert!(live_closure_item_matches_input(
            &item,
            &expected,
            "message-target"
        ));
    }

    #[test]
    fn live_closure_item_correlation_rejects_malformed_or_mismatched_function_outputs() {
        let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
        let valid_output = format!(
            "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>message-target</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
        );
        for item in [
            json!({
                "turnId": "turn-target",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": format!(
                    "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <input>{challenge}-other</input>\n</codex_delegation>"
                )
            }),
            json!({
                "turnId": "turn-target",
                "type": "functionCallOutput",
                "name": "other_tool",
                "output": valid_output
            }),
            json!({
                "turnId": "turn-target",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": format!("prefix\n{valid_output}")
            }),
            json!({
                "turnId": "turn-target",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": format!(
                    "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <input>{challenge}</input>\n</codex_delegation>\nsuffix"
                )
            }),
            json!({
                "turnId": "turn-target",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <input></input>\n</codex_delegation>"
            }),
            json!({
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": valid_output
            }),
        ] {
            let expected = LiveClosureExpectedNativeInputs {
                exact: vec![challenge.to_owned()],
                batch_category: "closure".to_owned(),
            };
            assert!(!live_closure_item_matches_input(
                &item,
                &expected,
                "message-target"
            ));
        }

        let stale_output = format!(
            "<codex_delegation>\n  <source_thread_id>source-thread</source_thread_id>\n  <client_message_id>message-old</client_message_id>\n  <input>{challenge}</input>\n</codex_delegation>"
        );
        let expected = LiveClosureExpectedNativeInputs {
            exact: vec![challenge.to_owned()],
            batch_category: "closure".to_owned(),
        };
        assert!(!live_closure_item_matches_input(
            &json!({
                "turnId": "turn-target",
                "type": "functionCallOutput",
                "name": "send_message_to_thread",
                "output": stale_output,
            }),
            &expected,
            "message-target"
        ));
    }

    #[test]
    fn live_closure_full_turn_items_preserve_exact_message_turn_result_correlation() {
        let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
        let turns = vec![
            json!({
                "id": "turn-other",
                "status": "completed",
                "items": [{
                    "type": "userMessage",
                    "clientId": "collab-notification-message-other",
                    "content": [{"type": "text", "text": challenge}]
                }, {
                    "type": "agentMessage",
                    "id": "result-other"
                }]
            }),
            json!({
                "id": "turn-target",
                "status": "completed",
                "items": [{
                    "type": "userMessage",
                    "clientId": "collab-notification-message-target",
                    "content": [{"type": "text", "text": challenge}]
                }, {
                    "type": "agentMessage",
                    "id": "result-target"
                }]
            }),
        ];
        let items = live_closure_turn_items(&turns).unwrap();
        let input = items
            .iter()
            .find(|item| {
                live_closure_item_message_id(item) == Some("message-target")
                    && live_closure_item_contains_challenge(item, challenge)
            })
            .unwrap();
        let input_turn_id = live_closure_item_turn_id(input).unwrap();
        assert_eq!(input_turn_id, "turn-target");
        let completed_turn = turns
            .iter()
            .find(|turn| {
                turn["status"] == "completed" && turn["id"].as_str() == Some(input_turn_id)
            })
            .unwrap();
        assert_eq!(completed_turn["id"], "turn-target");
        let result = items
            .iter()
            .rev()
            .find(|item| {
                live_closure_item_payload(item)["id"] == "result-target"
                    && live_closure_item_turn_id(item) == Some("turn-target")
            })
            .unwrap();
        assert_eq!(live_closure_item_payload(result)["id"], "result-target");
    }

    #[test]
    fn live_closure_full_turn_items_reject_malformed_history() {
        for malformed in [
            json!([{"status": "completed", "items": []}]),
            json!([{"id": "turn-1", "status": "completed"}]),
            json!([{
                "id": "turn-1",
                "status": "completed",
                "items": ["not-an-item"]
            }]),
            json!([{
                "id": "turn-1",
                "status": "completed",
                "items": [{"turnId": 7}]
            }]),
            json!([{
                "id": "turn-1",
                "status": "completed",
                "items": [{"item": "not-an-envelope"}]
            }]),
        ] {
            assert!(
                live_closure_turn_items(malformed.as_array().unwrap()).is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn live_closure_full_turn_items_reject_mismatched_history() {
        for mismatched in [
            json!({
                "turnId": "turn-other",
                "type": "userMessage",
                "clientId": "collab-notification-message-target",
                "content": [{"type": "text", "text": "challenge"}]
            }),
            json!({
                "turnId": "turn-target",
                "item": {
                    "turnId": "turn-other",
                    "type": "userMessage",
                    "clientId": "collab-notification-message-target",
                    "content": [{"type": "text", "text": "challenge"}]
                }
            }),
        ] {
            let error = live_closure_turn_items(&[json!({
                "id": "turn-target",
                "status": "completed",
                "items": [mismatched]
            })])
            .unwrap_err();
            assert!(error
                .to_string()
                .contains("COLLAB_LIVE_CLOSURE_TARGET_ITEM_TURN_MISMATCH"));
        }
    }

    #[test]
    fn live_closure_fresh_thread_materialization_is_bounded_pending() {
        let attempts = std::cell::Cell::new(0);
        let execution = wait_live_closure_fresh_thread_materialization(
            || {
                let attempt = attempts.get();
                attempts.set(attempt + 1);
                if attempt == 0 {
                    return Err(live_closure_turns_read_error(
                        client::adapters::AdapterError::Unknown {
                            operation: "rpc",
                            detail: "list_turns is not supported yet".into(),
                        },
                    ));
                }
                Ok(json!({"status": "completed"}))
            },
            Instant::now() + Duration::from_millis(500),
            Duration::from_millis(1),
        )
        .unwrap();

        assert_eq!(execution, json!({"status": "completed"}));
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn live_closure_permanent_unsupported_turns_read_fails_closed() {
        let attempts = std::cell::Cell::new(0);
        let error = wait_live_closure_fresh_thread_materialization(
            || {
                attempts.set(attempts.get() + 1);
                Err(live_closure_turns_read_error(
                    client::adapters::AdapterError::Unknown {
                        operation: "rpc",
                        detail: "thread/turns/list is not supported yet".into(),
                    },
                ))
            },
            Instant::now() + Duration::from_millis(500),
            Duration::from_millis(1),
        )
        .unwrap_err();

        assert_eq!(attempts.get(), 1);
        assert!(error
            .to_string()
            .starts_with("COLLAB_LIVE_CLOSURE_TARGET_TURNS_READ:"));
        assert!(error
            .to_string()
            .contains("thread/turns/list is not supported yet"));
    }

    #[test]
    fn live_closure_item_correlation_requires_the_exact_native_challenge_body() {
        let challenge = "appsdk-collab-live:closure-1:peer_to_peer";
        let item = json!({
            "turnId": "turn-envelope",
            "item": {
                "type": "userMessage",
                "clientId": "collab-notification-message-envelope",
                "content": [{"type": "text", "text": challenge }]
            }
        });
        assert!(live_closure_item_contains_challenge(&item, challenge));

        let wrapped = json!({
            "turnId": "turn-envelope",
            "item": {
                "type": "userMessage",
                "clientId": "collab-notification-message-envelope",
                "content": [{"type": "text", "text": format!("{challenge} | READ IS NOT DONE") }]
            }
        });
        assert!(!live_closure_item_contains_challenge(&wrapped, challenge));

        let mismatched = json!({
            "turnId": "turn-envelope",
            "item": {
                "type": "userMessage",
                "clientId": "collab-notification-message-envelope",
                "content": [{"type": "text", "text": "appsdk-collab-live:closure-1:other-path" }]
            }
        });
        assert!(!live_closure_item_contains_challenge(
            &mismatched,
            challenge
        ));
    }

    #[test]
    fn live_closure_pages_follow_backwards_cursor_to_find_over_window_challenge() {
        let requested_cursors = std::cell::RefCell::new(Vec::new());
        let pages = [
            json!({
                "data": [{"id": "recent-item"}],
                "backwardsCursor": "older-page"
            }),
            json!({
                "data": [{"id": "exact-challenge-item"}],
                "backwardsCursor": null
            }),
        ];
        let values = read_live_closure_pages(|cursor| {
            requested_cursors
                .borrow_mut()
                .push(cursor.map(str::to_owned));
            Ok(pages[requested_cursors.borrow().len() - 1].clone())
        })
        .unwrap();

        assert_eq!(
            requested_cursors.into_inner(),
            vec![None, Some("older-page".into())]
        );
        assert_eq!(
            values
                .iter()
                .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>(),
            vec!["recent-item", "exact-challenge-item"]
        );
    }

    #[test]
    fn live_closure_pages_use_next_cursor_and_bound_repeated_cursors() {
        let requested_cursors = std::cell::RefCell::new(Vec::new());
        let pages = [
            json!({"data": [{"id": "turn-1"}], "nextCursor": "page-2"}),
            json!({"data": [{"id": "turn-2"}], "nextCursor": "page-2"}),
        ];
        let values = read_live_closure_pages(|cursor| {
            requested_cursors
                .borrow_mut()
                .push(cursor.map(str::to_owned));
            Ok(pages[requested_cursors.borrow().len() - 1].clone())
        })
        .unwrap();
        assert_eq!(
            values,
            vec![json!({"id": "turn-1"}), json!({"id": "turn-2"})]
        );
        assert_eq!(
            requested_cursors.into_inner(),
            vec![None, Some("page-2".into())]
        );
    }

    #[test]
    fn live_closure_timeout_is_bounded_and_configurable() {
        assert_eq!(
            live_closure_timeout_from_value(None).unwrap(),
            Duration::from_millis(DEFAULT_LIVE_CLOSURE_TIMEOUT_MS)
        );
        assert_eq!(
            live_closure_timeout_from_value(Some("240000")).unwrap(),
            Duration::from_millis(240_000)
        );
        assert_eq!(
            live_closure_timeout_from_value(Some("3600000")).unwrap(),
            Duration::from_millis(MAX_LIVE_CLOSURE_TIMEOUT_MS)
        );
        assert!(live_closure_timeout_from_value(Some("0"))
            .unwrap_err()
            .to_string()
            .contains("COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID"));
        assert!(live_closure_timeout_from_value(Some("3600001"))
            .unwrap_err()
            .to_string()
            .contains("COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID"));
        assert!(live_closure_timeout_from_value(Some("not-a-duration"))
            .unwrap_err()
            .to_string()
            .contains("COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID"));
    }

    #[test]
    fn live_closure_receipt_wait_rechecks_until_consume_before_deadline() {
        let attempts = std::cell::Cell::new(0);
        let receipt = wait_live_closure_receipt(
            || {
                let attempt = attempts.get();
                attempts.set(attempt + 1);
                Ok(if attempt == 0 {
                    json!({"id": "message-1", "state": "pending", "body": "challenge"})
                } else {
                    json!({"id": "message-1", "state": "read", "body": "challenge"})
                })
            },
            Instant::now() + Duration::from_millis(500),
            "message-1",
            "challenge",
        )
        .unwrap();
        assert_eq!(receipt["state"], "read");
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn cli_project_context_uses_the_cli_app_and_exact_root() {
        let root = test_root("project-context");
        let context = cli_project_context(&root).unwrap();
        let canonical = root.canonicalize().unwrap();
        assert_eq!(context.app_scope_id.as_str(), identity::CLI_APP_SERVER_ID);
        assert_eq!(context.canonical_root, canonical.to_string_lossy());
        assert_eq!(context.project_scope.as_str(), canonical.to_string_lossy());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn default_recv_publishes_a_replayable_receive_identity_before_consuming() {
        let first = new_receive_id();
        let second = new_receive_id();
        assert_ne!(
            first, second,
            "each default receive owns a distinct identity"
        );
        assert!(
            crate::identity::validate_id_for_protocol(&first).is_ok(),
            "the generated identity must satisfy protocol identifier rules"
        );
        let banner = receive_recovery_banner(&first);
        assert!(banner.contains(&format!("receive_id={first}")));
        assert!(
            banner.contains(&format!("collab recv --receive-id {first}")),
            "the default path must publish the exact replay command: {banner}"
        );
    }

    #[test]
    fn default_recv_aborts_before_dispatch_when_identity_publication_fails() {
        struct FailingStderr;
        impl std::io::Write for FailingStderr {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "stderr is not writable",
                ))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let dispatched = std::cell::Cell::new(false);
        let error =
            recv_with_published_identity(&mut FailingStderr, "receive-publish-fail-1", |_| {
                dispatched.set(true);
                Ok(json!({"consumed": true}))
            })
            .unwrap_err();
        assert!(
            !dispatched.get(),
            "an unpublished receive identity must abort before Poll dispatch"
        );
        assert!(
            error
                .to_string()
                .starts_with("RECEIVE_IDENTITY_PUBLISH_FAILED"),
            "the publication failure must be the first reported error: {error}"
        );
    }

    #[test]
    fn default_recv_publishes_the_replay_identity_before_dispatch() {
        struct RecordingStderr {
            published: std::rc::Rc<std::cell::RefCell<String>>,
            flushed: std::rc::Rc<std::cell::Cell<bool>>,
        }
        impl std::io::Write for RecordingStderr {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.published
                    .borrow_mut()
                    .push_str(&String::from_utf8_lossy(buf));
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushed.set(true);
                Ok(())
            }
        }
        let published = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let flushed = std::rc::Rc::new(std::cell::Cell::new(false));
        let mut stderr = RecordingStderr {
            published: published.clone(),
            flushed: flushed.clone(),
        };
        let observed = published.clone();
        let observed_flushed = flushed.clone();
        let error = recv_with_published_identity(
            &mut stderr,
            "receive-publish-order-1",
            move |receive_id| {
                let visible = observed.borrow().clone();
                assert!(
                    observed_flushed.get(),
                    "the replay identity must be flushed before the request is dispatched"
                );
                assert!(
                    visible.contains(&format!("receive_id={receive_id}")),
                    "the replay identity must be visible before dispatch: {visible}"
                );
                Err(anyhow::anyhow!("ADAPTER_TIMEOUT: original transport error"))
            },
        )
        .unwrap_err();
        assert!(
            error.to_string().starts_with("ADAPTER_TIMEOUT"),
            "the original dispatch error must be the first reported error: {error}"
        );
    }

    #[test]
    fn context_root_resolution_fails_closed_without_route_or_baseline() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("context-root-unresolved");
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let host_paths = scope::HostPaths::resolve().unwrap();
        let error = resolve_context_root(&host_paths, &root)
            .err()
            .expect("no route and no baseline must fail closed");
        std::env::set_current_dir(previous).unwrap();
        assert!(
            error.to_string().starts_with("COLLAB_CONTEXT_UNRESOLVED:"),
            "{error:#}"
        );
        assert!(error.to_string().contains("`collab context`"));
        assert!(!root.join(".agent-collab").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn context_root_resolution_prefers_cwd_baseline_for_local_project() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("context-root-cwd");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let host_paths = scope::HostPaths::resolve().unwrap();
        let (scope, resolution) = resolve_context_root(&host_paths, &root).unwrap();
        std::env::set_current_dir(previous).unwrap();
        assert!(
            matches!(resolution, "route" | "cwd"),
            "expected route or cwd, got {resolution}"
        );
        assert_eq!(scope.root, root.canonicalize().unwrap());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn context_daemon_down_marker_fails_closed_and_preserves_marker() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("context-down-marker");
        let state_root = std::env::temp_dir().join(format!(
            "cs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::fs::write(state_root.join("DOWN"), "explicit down\n").unwrap();
        let previous_state = std::env::var_os(crate::scope::COLLAB_STATE_DIR_ENV);
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);

        let scope = Scope { root: root.clone() };
        let error = client::ensure_server(&scope.sock_path()).unwrap_err();
        assert!(
            error.to_string().starts_with("DAEMON_UNAVAILABLE:"),
            "{error:#}"
        );
        assert!(state_root.join("DOWN").is_file());

        match previous_state {
            Some(value) => std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, value),
            None => std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV),
        }
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cli_error_decorates_route_resolve_not_found_from_current_and_legacy_daemons() {
        for error in [
            "ROUTE_RESOLVE_NOT_FOUND: no registered Collab route is bound to App Server thread thread-old-daemon",
        ] {
            let formatted = format_cli_error(error);
            assert!(formatted.starts_with(error), "{formatted}");
            for expected in [
                "`collab context`",
            ] {
                assert!(
                    formatted.contains(expected),
                    "missing {expected}: {formatted}"
                );
            }
            for forbidden in [
                "`collab down`",
                "`collab up` once",
                "`appsdk init .`",
                "`collab route resolve --pane-id <pane-id>`",
                "`collab master status`",
            ] {
                assert!(
                    !formatted.contains(forbidden),
                    "forbidden recovery step {forbidden}: {formatted}"
                );
            }
            assert_eq!(
                format_cli_error(&formatted),
                formatted,
                "recovery guidance must not be duplicated"
            );
        }

        let current = format!(
            "ROUTE_RESOLVE_NOT_FOUND: no registered Collab route is bound to App Server thread thread-current-daemon; {}",
            crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY
        );
        assert_eq!(format_cli_error(&current), current);
        assert_eq!(
            current
                .matches(crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY)
                .count(),
            1,
            "{current}"
        );
    }

    #[test]
    fn cli_error_decorates_identity_rebind_and_cross_project_with_manual_steps() {
        for error in [
            "IDENTITY_REBIND_UNPROVEN: existing peer codex-%4 does not match the current pane",
            "IDENTITY_RESTORE_CROSS_PROJECT: a unique tmux/Codex anchor belongs to another project",
        ] {
            let formatted = format_cli_error(error);
            assert!(formatted.starts_with(error), "{formatted}");
            for expected in [
                "`collab context`",
                "canonical project main checkout",
                "live master",
            ] {
                assert!(
                    formatted.contains(expected),
                    "missing {expected}: {formatted}"
                );
            }
            for forbidden in ["do not edit routes", "copy tokens", "start a second daemon"] {
                assert!(
                    formatted.contains(forbidden),
                    "missing forbidden recovery step {forbidden}: {formatted}"
                );
            }
            assert_eq!(
                format_cli_error(&formatted),
                formatted,
                "recovery guidance must not be duplicated"
            );
        }
    }

    #[test]
    fn ensure_registration_rejects_a_retired_appserver_runtime() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("registration-reuse");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": root.canonicalize().unwrap(),
            "canonical_root": root.canonicalize().unwrap(),
            "storage_root": root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
        let runtime = RuntimeIdentity::cli_adapter("worker-1").unwrap();
        let mut identity = identity_with_runtime(Some(runtime));
        identity.project_scope = Some(
            Scope { root: root.clone() }
                .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
                .unwrap()
                .project_scope_id,
        );
        identity.transport = Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-worker-1".into()),
            thread_id: Some("thread-worker-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test appserver".into(),
        });
        assert!(
            !persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap()
        );
        std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_identity_without_project_scope_requires_registration() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("registration-legacy-scope");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": root.canonicalize().unwrap(),
            "canonical_root": root.canonicalize().unwrap(),
            "storage_root": root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
        let runtime = RuntimeIdentity::cli_adapter("worker-1").unwrap();
        let identity = identity_with_runtime(Some(runtime));

        assert!(
            !persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap()
        );

        std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ensure_registration_rebinds_a_thread_bound_to_another_project() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("registration-rebind");
        let old_root = root.join("old-project");
        let new_root = root.join("new-project");
        let state_root = root.join("global");
        std::fs::create_dir_all(&old_root).unwrap();
        std::fs::create_dir_all(&new_root).unwrap();
        std::fs::create_dir_all(old_root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(new_root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": old_root.canonicalize().unwrap(),
            "canonical_root": old_root.canonicalize().unwrap(),
            "storage_root": old_root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
        let runtime = RuntimeIdentity::cli_adapter("worker-1").unwrap();
        let mut identity = identity_with_runtime(Some(runtime));
        identity.project_scope = Some(
            Scope {
                root: old_root.clone(),
            }
            .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
            .unwrap()
            .project_scope_id,
        );
        identity.transport = Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-worker-1".into()),
            thread_id: Some("thread-worker-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test appserver".into(),
        });

        assert!(!persisted_runtime_matches_scope(
            &Scope {
                root: new_root.clone()
            },
            &identity
        )
        .unwrap());
        // A retired AppServer transport is not reusable even in its original
        // project; the next mutating command must bind the current tmux pane.
        assert!(!persisted_runtime_matches_scope(
            &Scope {
                root: old_root.clone()
            },
            &identity
        )
        .unwrap());

        std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_thread_only_identity_is_not_reusable_and_must_re_register() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("registration-legacy-thread-only");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": root.canonicalize().unwrap(),
            "canonical_root": root.canonicalize().unwrap(),
            "storage_root": root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        // The pre-dual-key durable shape: a native thread with no session on
        // either the runtime or the selected transport.
        let runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("worker-1").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-legacy").unwrap(),
            appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
            endpoint_generation: 1,
            binding_id: identity::BindingId::new("binding-legacy").unwrap(),
            session_id: None,
            native_thread_id: Some(identity::NativeThreadId::new("thread-legacy").unwrap()),
        };
        let mut identity = identity_with_runtime(Some(runtime));
        identity.project_scope = Some(
            Scope { root: root.clone() }
                .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
                .unwrap()
                .project_scope_id,
        );
        identity.transport = Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: None,
            thread_id: Some("thread-legacy".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test appserver".into(),
        });

        // A recovered legacy identity must not be reported as reusable: it
        // cannot resolve its own route until it re-registers with the host
        // session and upgrades to the strict dual-key binding.
        assert!(
            !persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap()
        );

        std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
        std::fs::remove_dir_all(root).ok();
    }

    /// `collab init` must not claim a persisted peer without a stable identity
    /// anchor, even when that peer is the only identity in the project.
    #[test]
    fn init_refuses_to_rebind_a_legacy_identity_without_a_matching_anchor() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("init-scope-rebind");
        // The daemon socket lives in the state root, so keep that path short
        // enough for `sockaddr_un`.
        let state_root = std::env::temp_dir().join(format!(
            "cs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        let tmux_fixture = crate::server::peer_tests::IsolatedTmux::start_single(&root);
        let current_endpoint = tmux_fixture.endpoints().remove(0);
        let previous_tmux = std::env::var_os("TMUX");
        let previous_pane = std::env::var_os("TMUX_PANE");
        std::env::set_var(
            "TMUX",
            format!(
                "{},{},0",
                current_endpoint.socket_path, current_endpoint.server_pid
            ),
        );
        std::env::set_var("TMUX_PANE", &current_endpoint.pane_id);
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": root.canonicalize().unwrap(),
            "canonical_root": root.canonicalize().unwrap(),
            "storage_root": root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let old_runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("agent-peer").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-agent-peer").unwrap(),
            appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
            endpoint_generation: 1,
            binding_id: identity::BindingId::new("binding-agent-peer").unwrap(),
            session_id: Some(identity::SessionId::new("session-old").unwrap()),
            native_thread_id: Some(identity::NativeThreadId::new("thread-old").unwrap()),
        };
        let transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-old".into()),
            thread_id: Some("thread-old".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test appserver".into(),
        };
        let persisted = Identity {
            worker_id: "agent-peer".into(),
            token: "token-agent-peer".into(),
            project_scope: Some(
                Scope { root: root.clone() }
                    .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
                    .unwrap()
                    .project_scope_id,
            ),
            runtime: Some(old_runtime),
            transport: Some(transport.clone()),
        };
        let identity_path = state_root
            .join("identities")
            .join("agent-peer")
            .join("identity.json");
        std::fs::create_dir_all(identity_path.parent().unwrap()).unwrap();
        std::fs::write(
            &identity_path,
            serde_json::to_string_pretty(&persisted).unwrap(),
        )
        .unwrap();

        // The route listener proves that init refuses before asking the daemon
        // to resolve or re-register an unanchored legacy peer.
        let socket = state_root.join("server.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let root_string = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let request_observed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let request_observed_by_server = request_observed.clone();
        let responder = std::thread::spawn(move || {
            use std::io::{BufRead, Write};

            let (mut route_stream, _) = match listener.accept() {
                Ok(connection) => {
                    request_observed_by_server.store(true, std::sync::atomic::Ordering::SeqCst);
                    connection
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(error) => panic!("route test listener failed: {error}"),
            };
            let mut route_line = String::new();
            std::io::BufReader::new(&route_stream)
                .read_line(&mut route_line)
                .unwrap();
            let route_request: serde_json::Value = serde_json::from_str(&route_line).unwrap();
            assert_eq!(route_request["op"], "RouteResolve");
            assert_eq!(route_request["session_id"], "session-old");
            assert_eq!(route_request["native_thread_id"], "thread-old");
            route_stream
                .write_all(
                    format!(
                        "{}\n",
                        json!({
                            "ok": false,
                            "error": "ROUTE_RESOLVE_NOT_FOUND: no registered Collab route is bound to App Server thread"
                        })
                    )
                    .as_bytes(),
                )
                .unwrap();

            // 2. Registration must now arrive as the restored identity.
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            // The restored identity must register as itself, with its own token
            // and its own derived binding id.
            assert_eq!(request["worker_id"], "agent-peer");
            assert_eq!(request["token"], "token-agent-peer");
            let response = json!({
                "ok": true,
                "worker_id": "agent-peer",
                "identity_kind": "peer",
                "transport_selected": {
                    "kind": "appserver",
                    "endpoint": "unix:///tmp/codex.sock",
                    "namespace": "codex_tui",
                    "session_id": "session-new",
                    "thread_id": "thread-new",
                    "capabilities": ["send_message_to_thread"],
                    "self_check": "test appserver"
                },
                "typed": true,
                "command_id": "command-register",
                "operation_id": "operation-register",
                "sequence": 2,
                "revision": 2,
                "replayed": false,
                "command": {
                    "binding": {
                        "project_scope": root_string,
                        "app_scope_id": identity::CLI_APP_SERVER_ID,
                        "agent_id": "agent-peer",
                        "runtime_id": "runtime-agent-peer",
                        "binding_id": "binding-agent-peer",
                        "endpoint_generation": 2,
                        "session_id": "session-new",
                        "native_thread_id": "thread-new"
                    }
                }
            });
            std::io::Write::write_all(&mut stream, format!("{response}\n").as_bytes()).unwrap();
        });

        set_current_session_thread("thread-new", "session-new");
        let result = identity::load_or_create_for_init(&Scope { root: root.clone() }, None);
        responder.join().unwrap();
        let error = result.unwrap_err();
        assert!(
            error.to_string().starts_with("IDENTITY_REBIND_UNPROVEN:"),
            "unexpected init result: {error:#}"
        );
        assert!(!request_observed.load(std::sync::atomic::Ordering::SeqCst));
        clear_current_session_thread();
        match previous_pane {
            Some(value) => std::env::set_var("TMUX_PANE", value),
            None => std::env::remove_var("TMUX_PANE"),
        }
        match previous_tmux {
            Some(value) => std::env::set_var("TMUX", value),
            None => std::env::remove_var("TMUX"),
        }
        std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ensure_registration_uses_provisional_runtime_for_scope_mismatch() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("registration-recovery");
        let old_root = root.join("old-project");
        let new_root = root.join("new-project");
        let state_root = std::env::temp_dir().join(format!(
            "collab-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&old_root).unwrap();
        std::fs::create_dir_all(new_root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-old",
            "project_scope": old_root.canonicalize().unwrap(),
            "canonical_root": old_root.canonicalize().unwrap(),
            "storage_root": old_root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let old_runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("worker-1").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-old").unwrap(),
            appserver_id: identity::AppServerId::new("appserver-old").unwrap(),
            endpoint_generation: 3,
            binding_id: identity::BindingId::new("binding-old").unwrap(),
            session_id: Some(identity::SessionId::new("session-thread-old").unwrap()),
            native_thread_id: Some(identity::NativeThreadId::new("thread-old").unwrap()),
        };
        let mut identity = identity_with_runtime(Some(old_runtime));
        identity.project_scope = Some(
            Scope {
                root: old_root.clone(),
            }
            .route_scope(identity::AppServerId::new("appserver-old").unwrap())
            .unwrap()
            .project_scope_id,
        );
        identity.transport = Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-thread-old".into()),
            thread_id: Some("thread-old".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test appserver".into(),
        });

        let socket = state_root.join("server.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let new_root_string = new_root
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let responder = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                request["project_context"]["app_scope_id"],
                identity::CLI_APP_SERVER_ID
            );
            let response = json!({
                "ok": true,
                "worker_id": "worker-1",
                "identity_kind": "peer",
                "transport_selected": {
                    "kind": "appserver",
                    "endpoint": "unix:///tmp/codex.sock",
                    "namespace": "codex_tui",
                    "session_id": "session-thread-new",
                    "thread_id": "thread-new",
                    "capabilities": ["send_message_to_thread"],
                    "self_check": "test appserver"
                },
                "typed": true,
                "command_id": "command-register",
                "operation_id": "operation-register",
                "sequence": 1,
                "revision": 1,
                "replayed": false,
                "command": {
                    "binding": {
                        "project_scope": new_root_string,
                        "app_scope_id": identity::CLI_APP_SERVER_ID,
                        "agent_id": "worker-1",
                        "runtime_id": "runtime-new",
                        "binding_id": "binding-new",
                        "endpoint_generation": 1,
                        "session_id": "session-thread-new",
                        "native_thread_id": "thread-new"
                    }
                }
            });
            std::io::Write::write_all(&mut stream, format!("{response}\n").as_bytes()).unwrap();
        });

        let response = ensure_registration(
            &Scope {
                root: new_root.clone(),
            },
            &mut identity,
        )
        .unwrap();
        responder.join().unwrap();
        assert_eq!(response["typed"], true);
        assert_eq!(
            identity.runtime.unwrap().appserver_id.as_str(),
            identity::CLI_APP_SERVER_ID
        );
        std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// An ordinary command cannot adopt a persisted peer without a matching
    /// pane, session, or thread anchor.
    #[test]
    fn ordinary_command_refuses_an_unanchored_legacy_identity() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("ordinary-registration-rebind");
        let state_root = std::env::temp_dir().join(format!(
            "cs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        let tmux_fixture = crate::server::peer_tests::IsolatedTmux::start_single(&root);
        let current_endpoint = tmux_fixture.endpoints().remove(0);
        let previous_tmux = std::env::var_os("TMUX");
        let previous_pane = std::env::var_os("TMUX_PANE");
        std::env::set_var(
            "TMUX",
            format!(
                "{},{},0",
                current_endpoint.socket_path, current_endpoint.server_pid
            ),
        );
        std::env::set_var("TMUX_PANE", &current_endpoint.pane_id);
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": root.canonicalize().unwrap(),
            "canonical_root": root.canonicalize().unwrap(),
            "storage_root": root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let old_runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("agent-peer").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-agent-peer").unwrap(),
            appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
            endpoint_generation: 1,
            binding_id: identity::BindingId::new("binding-agent-peer").unwrap(),
            session_id: Some(identity::SessionId::new("session-old").unwrap()),
            native_thread_id: Some(identity::NativeThreadId::new("thread-old").unwrap()),
        };
        let persisted = Identity {
            worker_id: "agent-peer".into(),
            token: "token-agent-peer".into(),
            project_scope: Some(
                Scope { root: root.clone() }
                    .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
                    .unwrap()
                    .project_scope_id,
            ),
            runtime: Some(old_runtime),
            transport: Some(SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-old".into()),
                thread_id: Some("thread-old".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message_to_thread".into()],
                self_check: "test appserver".into(),
            }),
        };
        let identity_path = state_root
            .join("identities")
            .join("agent-peer")
            .join("identity.json");
        std::fs::create_dir_all(identity_path.parent().unwrap()).unwrap();
        std::fs::write(
            &identity_path,
            serde_json::to_string_pretty(&persisted).unwrap(),
        )
        .unwrap();

        let socket = state_root.join("server.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let root_string = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let request_observed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let request_observed_by_server = request_observed.clone();
        let responder = std::thread::spawn(move || {
            use std::io::{BufRead, Write};

            let (mut route_stream, _) = match listener.accept() {
                Ok(connection) => {
                    request_observed_by_server.store(true, std::sync::atomic::Ordering::SeqCst);
                    connection
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(error) => panic!("route test listener failed: {error}"),
            };
            let mut route_line = String::new();
            std::io::BufReader::new(&route_stream)
                .read_line(&mut route_line)
                .unwrap();
            let route_request: serde_json::Value = serde_json::from_str(&route_line).unwrap();
            assert_eq!(route_request["op"], "RouteResolve");
            assert_eq!(route_request["session_id"], "session-old");
            assert_eq!(route_request["native_thread_id"], "thread-old");
            route_stream
                .write_all(
                    format!(
                        "{}\n",
                        json!({
                            "ok": false,
                            "error": "ROUTE_RESOLVE_NOT_FOUND: no registered Collab route is bound to App Server thread"
                        })
                    )
                    .as_bytes(),
                )
                .unwrap();

            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["worker_id"], "agent-peer");
            assert_eq!(request["token"], "token-agent-peer");
            let response = json!({
                "ok": true,
                "worker_id": "agent-peer",
                "identity_kind": "peer",
                "transport_selected": {
                    "kind": "appserver",
                    "endpoint": "unix:///tmp/codex.sock",
                    "namespace": "codex_tui",
                    "session_id": "session-new",
                    "thread_id": "thread-new",
                    "capabilities": ["send_message_to_thread"],
                    "self_check": "test appserver"
                },
                "typed": true,
                "command_id": "command-register",
                "operation_id": "operation-register",
                "sequence": 2,
                "revision": 2,
                "replayed": false,
                "command": {
                    "binding": {
                        "project_scope": root_string,
                        "app_scope_id": identity::CLI_APP_SERVER_ID,
                        "agent_id": "agent-peer",
                        "runtime_id": "runtime-agent-peer",
                        "binding_id": "binding-agent-peer",
                        "endpoint_generation": 2,
                        "session_id": "session-new",
                        "native_thread_id": "thread-new"
                    }
                }
            });
            std::io::Write::write_all(&mut stream, format!("{response}\n").as_bytes()).unwrap();
        });

        set_current_session_thread("thread-new", "session-new");
        let result = me(&Scope { root: root.clone() }, None);
        responder.join().unwrap();
        let error = result.unwrap_err();
        assert!(
            error.to_string().starts_with("IDENTITY_REBIND_UNPROVEN:"),
            "unexpected ordinary-command result: {error:#}"
        );
        assert!(!request_observed.load(std::sync::atomic::Ordering::SeqCst));
        clear_current_session_thread();
        match previous_pane {
            Some(value) => std::env::set_var("TMUX_PANE", value),
            None => std::env::remove_var("TMUX_PANE"),
        }
        match previous_tmux {
            Some(value) => std::env::set_var("TMUX", value),
            None => std::env::remove_var("TMUX"),
        }
        std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// Context may not restore an identity when the current invocation has no
    /// matching tmux pane, Codex session, or Codex thread anchor.
    #[test]
    fn context_route_miss_refuses_an_unproven_identity_restore() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("context-route-miss-rebind");
        let state_root = std::env::temp_dir().join(format!(
            "cs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": root.canonicalize().unwrap(),
            "canonical_root": root.canonicalize().unwrap(),
            "storage_root": root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let old_runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("agent-peer").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-agent-peer").unwrap(),
            appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
            endpoint_generation: 1,
            binding_id: identity::BindingId::new("binding-agent-peer").unwrap(),
            session_id: Some(identity::SessionId::new("session-old").unwrap()),
            native_thread_id: Some(identity::NativeThreadId::new("thread-old").unwrap()),
        };
        let persisted = Identity {
            worker_id: "agent-peer".into(),
            token: "token-agent-peer".into(),
            project_scope: Some(
                Scope { root: root.clone() }
                    .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
                    .unwrap()
                    .project_scope_id,
            ),
            runtime: Some(old_runtime),
            transport: Some(SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-old".into()),
                thread_id: Some("thread-old".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message_to_thread".into()],
                self_check: "test appserver".into(),
            }),
        };
        let identity_path = state_root
            .join("identities")
            .join("agent-peer")
            .join("identity.json");
        std::fs::create_dir_all(identity_path.parent().unwrap()).unwrap();
        std::fs::write(
            &identity_path,
            serde_json::to_string_pretty(&persisted).unwrap(),
        )
        .unwrap();

        let socket = state_root.join("server.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let root_string = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let request_observed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let request_observed_by_server = request_observed.clone();
        let responder = std::thread::spawn(move || {
            use std::io::{BufRead, Write};

            let (mut route_stream, _) = match listener.accept() {
                Ok(connection) => {
                    request_observed_by_server.store(true, std::sync::atomic::Ordering::SeqCst);
                    connection
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(error) => panic!("route test listener failed: {error}"),
            };
            let mut route_line = String::new();
            std::io::BufReader::new(&route_stream)
                .read_line(&mut route_line)
                .unwrap();
            let route_request: serde_json::Value = serde_json::from_str(&route_line).unwrap();
            assert_eq!(route_request["op"], "RouteResolve");
            assert_eq!(route_request["session_id"], "session-old");
            assert_eq!(route_request["native_thread_id"], "thread-old");
            route_stream
                .write_all(
                    format!(
                        "{}\n",
                        json!({
                            "ok": false,
                            "error": "ROUTE_RESOLVE_NOT_FOUND: no registered Collab route is bound to App Server thread"
                        })
                    )
                    .as_bytes(),
                )
                .unwrap();

            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["worker_id"], "agent-peer");
            assert_eq!(request["token"], "token-agent-peer");
            let response = json!({
                "ok": true,
                "worker_id": "agent-peer",
                "identity_kind": "peer",
                "transport_selected": {
                    "kind": "appserver",
                    "endpoint": "unix:///tmp/codex.sock",
                    "namespace": "codex_tui",
                    "session_id": "session-new",
                    "thread_id": "thread-new",
                    "capabilities": ["send_message_to_thread"],
                    "self_check": "test appserver"
                },
                "typed": true,
                "command_id": "command-register",
                "operation_id": "operation-register",
                "sequence": 2,
                "revision": 2,
                "replayed": false,
                "command": {
                    "binding": {
                        "project_scope": root_string,
                        "app_scope_id": identity::CLI_APP_SERVER_ID,
                        "agent_id": "agent-peer",
                        "runtime_id": "runtime-agent-peer",
                        "binding_id": "binding-agent-peer",
                        "endpoint_generation": 2,
                        "session_id": "session-new",
                        "native_thread_id": "thread-new"
                    }
                }
            });
            std::io::Write::write_all(&mut stream, format!("{response}\n").as_bytes()).unwrap();
        });

        set_current_session_thread("thread-new", "session-new");
        let previous_pane = std::env::var_os("TMUX_PANE");
        std::env::remove_var("TMUX_PANE");
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let recovery = identity::load_or_create(&Scope { root: root.clone() }, None, None);
        std::env::set_current_dir(previous).unwrap();
        if let Err(error) = recovery {
            assert!(
                error.to_string().starts_with("IDENTITY_REBIND_UNPROVEN:"),
                "unexpected recovery error: {error:#}"
            );
            responder.join().unwrap();
            assert!(!request_observed.load(std::sync::atomic::Ordering::SeqCst));
            clear_current_session_thread();
            match previous_pane {
                Some(value) => std::env::set_var("TMUX_PANE", value),
                None => std::env::remove_var("TMUX_PANE"),
            }
            std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
            std::fs::remove_dir_all(state_root).ok();
            std::fs::remove_dir_all(root).ok();
            return;
        }
        responder.join().unwrap();
        panic!("unproven identity must fail closed, got successful identity load");
    }

    #[test]
    fn init_runtime_projection_accepts_appserver_and_tmux_hosts() {
        let root = test_root("runtime-projection");
        let runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("worker-1").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-thread-1").unwrap(),
            appserver_id: identity::AppServerId::new("tui-default").unwrap(),
            endpoint_generation: 2,
            binding_id: identity::BindingId::new("binding-thread-1").unwrap(),
            session_id: Some(identity::SessionId::new("session-1").unwrap()),
            native_thread_id: Some(identity::NativeThreadId::new("thread-1").unwrap()),
        };
        let mut identity = identity_with_runtime(Some(runtime));
        identity.transport = Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into(), "read_thread".into()],
            self_check: "test appserver".into(),
        });
        let projection =
            registered_runtime_projection(&Scope { root: root.clone() }, &identity, 4242).unwrap();
        assert_eq!(projection["transport"], "appserver");
        assert_eq!(projection["threadId"], "thread-1");
        assert_eq!(projection["sessionId"], "session-1");
        assert_eq!(projection["endpoint"], "unix:///tmp/codex.sock");
        assert_eq!(projection["processId"], 4242);
        identity.transport = Some(SelectedTransport {
            kind: TransportKind::Tmux,
            endpoint: Some("/tmp/tmux-test.sock".into()),
            namespace: Some("$1".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: Some(proto::TmuxEndpoint {
                socket_path: "/tmp/tmux-test.sock".into(),
                server_pid: 42,
                tmux_session_id: "$1".into(),
                pane_id: "%1".into(),
                pane_pid: 43,
                codex_session_id: Some("session-1".into()),
                codex_thread_id: Some("thread-1".into()),
            }),
            capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
            self_check: "test tmux".into(),
        });
        let projection =
            registered_runtime_projection(&Scope { root: root.clone() }, &identity, 4242).unwrap();
        assert_eq!(projection["runtimeId"], "runtime-thread-1");
        assert_eq!(projection["appserverId"], "tui-default");
        assert_eq!(projection["transport"], "tmux");
        assert_eq!(projection["tmuxEndpoint"]["pane_id"], "%1");
        assert_eq!(
            projection["projectRoot"],
            root.canonicalize().unwrap().to_string_lossy().as_ref()
        );
        assert_eq!(projection["capabilities"][0], "send_message_to_pane");
        assert_eq!(projection["processId"], 4242);
        assert!(
            registered_runtime_projection(&Scope { root: root.clone() }, &identity, 0)
                .unwrap_err()
                .to_string()
                .contains("PID is zero")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn command_envelope_uses_the_registered_binding_and_generation() {
        let root = test_root("command-envelope");
        let runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("worker-1").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-live").unwrap(),
            appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
            endpoint_generation: 9,
            binding_id: identity::BindingId::new("binding-live").unwrap(),
            session_id: None,
            native_thread_id: None,
        };
        let identity = identity_with_runtime(Some(runtime));
        let envelope = command_envelope(&Scope { root: root.clone() }, &identity).unwrap();
        assert_eq!(envelope.actor_binding_id.as_str(), "binding-live");
        assert_eq!(envelope.endpoint_generation, 9);
        assert_eq!(
            envelope.scope.app_scope_id.as_str(),
            identity::CLI_APP_SERVER_ID
        );
        assert_eq!(
            envelope.scope.project_scope_id.as_str(),
            root.canonicalize().unwrap().to_string_lossy()
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn subagent_snapshot_uses_the_authenticated_mutation_route() {
        assert!(subagent_observe_query(&subagent::Action::List).is_some());
        assert!(subagent_observe_query(&subagent::Action::Status { id: "child".into() }).is_some());
        assert!(subagent_observe_query(&subagent::Action::Snapshot {
            id: "child".into(),
            lines: 40,
        })
        .is_none());
    }

    #[test]
    fn persisted_appserver_binding_requires_live_native_route_resolution() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("registration-appserver-live");
        // The daemon socket lives in the state root, so keep that path short
        // enough for `sockaddr_un`.
        let state_root = std::env::temp_dir().join(format!(
            "cs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        set_current_session_thread("thread-1", "session-1");
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": root.canonicalize().unwrap(),
            "canonical_root": root.canonicalize().unwrap(),
            "storage_root": root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("worker-1").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-live-1").unwrap(),
            appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
            endpoint_generation: 1,
            binding_id: identity::BindingId::new("binding-live-1").unwrap(),
            session_id: Some(identity::SessionId::new("session-1").unwrap()),
            native_thread_id: Some(identity::NativeThreadId::new("thread-1").unwrap()),
        };
        let mut identity = identity_with_runtime(Some(runtime));
        identity.project_scope = Some(
            Scope { root: root.clone() }
                .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
                .unwrap()
                .project_scope_id,
        );
        identity.transport = Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test appserver".into(),
        });

        let socket = Scope { root: root.clone() }.sock_path();
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let root_string = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let responder = std::thread::spawn(move || {
            use std::io::Write;
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["op"], "RouteResolveNative");
            assert_eq!(request["session_id"], "session-1");
            assert_eq!(request["native_thread_id"], "thread-1");
            let response = json!({
                "ok": true,
                "app_scope_id": identity::CLI_APP_SERVER_ID,
                "project_scope": root_string,
                "canonical_root": root_string,
                "storage_root": root_string,
                "agent_id": "worker-1",
                "binding_id": "binding-live-1",
                "endpoint_generation": 1,
                "session_id": "session-1",
                "native_thread_id": "thread-1"
            });
            stream
                .write_all(format!("{response}\n").as_bytes())
                .unwrap();
        });

        assert!(persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap());
        responder.join().unwrap();
        clear_current_session_thread();
        std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn persisted_binding_for_another_session_or_thread_is_not_reusable() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let root = test_root("registration-foreign-address");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
        set_current_session_thread("thread-current", "session-current");
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": root.canonicalize().unwrap(),
            "canonical_root": root.canonicalize().unwrap(),
            "storage_root": root.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
        let runtime = RuntimeIdentity {
            agent_id: identity::AgentId::new("worker-1").unwrap(),
            runtime_id: identity::RuntimeId::new("runtime-worker-1").unwrap(),
            appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
            endpoint_generation: 1,
            binding_id: identity::BindingId::new("binding-worker-1").unwrap(),
            session_id: Some(identity::SessionId::new("session-other").unwrap()),
            native_thread_id: Some(identity::NativeThreadId::new("thread-other").unwrap()),
        };
        let mut identity = identity_with_runtime(Some(runtime));
        identity.project_scope = Some(
            Scope { root: root.clone() }
                .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
                .unwrap()
                .project_scope_id,
        );
        identity.transport = Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-other".into()),
            thread_id: Some("thread-other".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test appserver".into(),
        });
        assert!(
            !persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap(),
            "a binding for another session/thread must not be reused"
        );
        clear_current_session_thread();
        std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
        std::fs::remove_dir_all(root).ok();
    }
}
