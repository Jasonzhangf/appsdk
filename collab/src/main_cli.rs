use super::board::BoardCommand;
use super::subagent::Action;
use clap::Subcommand;


#[derive(Subcommand)]
pub(crate) enum NotifyCmd {
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
pub(crate) enum TaskCmd {
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
    /// Accept a board invitation (with revision), or a legacy assigned task
    Accept {
        id: String,
        #[arg(long)]
        expected_revision: Option<u64>,
    },
    /// Decline an invitation or an unstarted legacy assignment
    Decline {
        id: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        reason: String,
        /// Explicitly reject a legacy assigned task rather than a board invite
        #[arg(long)]
        legacy_assignment: bool,
    },
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
pub(crate) enum MasterCmd {
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
pub(crate) enum RouteCmd {
    /// Resolve the route bound to the current tmux session and pane
    Resolve {
        #[arg(long = "tmux-session-id")]
        session_id: Option<String>,
        #[arg(long = "pane-id")]
        pane_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum LiveClosureCmd {
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
pub(crate) enum WorkerCmd {
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
pub(crate) enum MigrateCmd {
    /// Inspect current durable state and migration blockers
    Inspect,
    /// Create a migration plan; does not freeze admission
    Plan,
    /// Freeze task admission and persist a deterministic snapshot
    Apply,
    /// Verify replayed state and resume task admission
    Verify,
}


#[derive(Subcommand)]
pub(crate) enum Cmd {
    /// Managed persistent agent peers (current project only)
    Subagent {
        #[command(subcommand)]
        command: Action,
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
    /// Open a read-only loopback Web observer for this project's task board
    Dashboard {
        /// Loopback port; 0 lets the operating system select an available port
        #[arg(long, default_value_t = 0)]
        port: u16,
    },
    /// Shared public task board for the project master and independent peers
    Board {
        #[command(subcommand)]
        cmd: BoardCommand,
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