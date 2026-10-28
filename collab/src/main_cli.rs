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
        /// Caller-owned task IDs whose board patrol is being rearmed
        #[arg(long = "task-id")]
        task_ids: Vec<String>,
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
        #[arg(long, default_value_t = 604_800)]
        ttl_seconds: u64,
    },
    /// Cancel caller-owned board-idle tasks or the current master's exact project patrol
    Cancel {
        /// Board task IDs; only valid with --event board-idle
        #[arg(long = "task-id")]
        task_ids: Vec<String>,
        /// board-idle (default) or master-idle project-patrol
        #[arg(long, default_value = "board-idle")]
        event: String,
        /// Durable cancellation reason shown in notify status
        #[arg(long)]
        reason: Option<String>,
        /// Condition that restores the patrol
        #[arg(long = "resume-condition")]
        resume_condition: Option<String>,
    },
    /// List the caller's notification subscriptions
    Status,
}

#[derive(Subcommand)]
pub(crate) enum GoalCmd {
    /// Reconcile the caller's owned finite goal schedule without touching
    /// daemon-owned direct-message reception.
    Reconcile {
        #[arg(long)]
        goal_id: String,
        #[arg(long)]
        owner: String,
        /// active or ended
        #[arg(long)]
        desired: String,
        /// Absolute UTC epoch milliseconds for an active one-shot occurrence
        #[arg(long = "at-ms")]
        trigger_ms: Option<i64>,
        #[arg(long = "ttl-seconds")]
        ttl_seconds: Option<u64>,
        /// Caller-owned retained historical goal subject to retire alongside
        /// the canonical goal identity. Exact and stable-name scoped only.
        #[arg(long = "goal-subject")]
        goal_subject: Option<String>,
        #[arg(long)]
        reason: Option<String>,
    },
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
        /// Explicitly classify this as read-only work with no code candidate.
        #[arg(long, conflicts_with = "worktree")]
        no_candidate: bool,
        #[arg(long)]
        worktree: Option<String>,
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
    ///
    /// When the task was force-closed without a recorded integration commit,
    /// pass `--commit <sha>` with a commit reachable from `refs/heads/main`
    /// (for example the squash commit) to authorize branch deletion.
    FinalizeCleanup {
        id: String,
        #[arg(long)]
        commit: Option<String>,
    },
    /// Close a merged task and clean up its declared worktree/branch.
    /// With --force the current master may close any task. With no master
    /// assigned, the owner may close its task, or a registered peer may close an
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
    },
}

#[derive(Subcommand)]
pub(crate) enum MasterCmd {
    /// Promote this peer, replacing any recorded holder; requires the user's approval text
    Promote {
        #[arg(long)]
        approval: String,
    },
    /// Clear the current master authority; requires the user's approval text
    Clear {
        #[arg(long)]
        approval: String,
    },
    /// Delegate master authority to another registered peer (current master only)
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
    /// Hold or release periodic master-idle wakes for a genuine external gate
    Wake {
        #[command(subcommand)]
        command: MasterWakeCmd,
    },
    /// Show the current master, if any
    Status,
}

#[derive(Subcommand)]
pub(crate) enum MasterWakeCmd {
    /// Suppress periodic master-idle wakes until the gate is released or the TTL expires
    Hold {
        /// Gate and release condition
        #[arg(long)]
        reason: String,
        /// Hold duration in seconds (1..=604800)
        #[arg(long)]
        ttl_seconds: u64,
    },
    /// Release the active master-idle wake hold immediately
    Release,
}

#[derive(Subcommand)]
pub(crate) enum CeoCmd {
    /// Read the host-wide CEO grant and suggested default workspace
    Status,
    /// Promote the current registered agent to the host-wide CEO
    Promote {
        /// The user's exact approval text
        #[arg(long)]
        approval: String,
        /// Existing registered workspace; defaults to the current source root
        #[arg(long)]
        workspace: Option<std::path::PathBuf>,
    },
    /// Clear the host-wide CEO grant
    Clear {
        /// The user's exact approval text
        #[arg(long)]
        approval: String,
    },
    /// Send a durable request to the current master of another project
    Send {
        #[arg(long)]
        project: std::path::PathBuf,
        /// Stable recipient name; defaults to the target project's current master
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        subject: String,
        #[arg(long = "in-reply-to")]
        in_reply_to: Option<String>,
        /// Track an obligation: the recipient must reply to this message.
        #[arg(long, conflicts_with = "reply_on_completion")]
        reply_required: bool,
        /// Track an obligation: reply once the delegated work is complete.
        #[arg(long, conflicts_with = "reply_required")]
        reply_on_completion: bool,
        #[arg(required = true, trailing_var_arg = true)]
        body: Vec<String>,
    },
    /// Create a board task owned by the target project's current master
    AppendTask {
        #[arg(long)]
        project: std::path::PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: String,
        #[arg(long = "delivery-condition")]
        delivery_condition: String,
        #[arg(long = "test-condition")]
        test_condition: String,
        /// Owner-local priority: p0 (highest) through p4
        #[arg(long, default_value = "p1")]
        priority: String,
        /// Track an obligation: the recipient must reply to this request.
        #[arg(long, conflicts_with = "reply_on_completion")]
        reply_required: bool,
        /// Track an obligation: reply once the delegated work is complete.
        #[arg(long, conflicts_with = "reply_required")]
        reply_on_completion: bool,
    },
    /// Reply as the current project master to the current host-wide CEO
    Reply {
        #[arg(long)]
        subject: String,
        #[arg(long = "in-reply-to")]
        in_reply_to: Option<String>,
        /// Mark this progress as explicitly urgent; urgent reports bypass the
        /// ordinary hourly/quiet-window limits. Never inferred from body text.
        #[arg(long)]
        urgent: bool,
        #[arg(required = true, trailing_var_arg = true)]
        body: Vec<String>,
    },
    /// Read the CEO grant, target masters, boards and pending reply markers
    Progress {
        /// Limit progress to one registered project
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
    /// Acknowledge a prepared CEO report batch after the report was visibly output
    ReportAck {
        /// Exact batch ID returned by `collab ceo progress`
        #[arg(long = "batch-id")]
        batch_id: String,
    },
    /// Prepare and launch one real Codex master in an existing tmux endpoint
    BootstrapMaster {
        #[arg(long)]
        project: std::path::PathBuf,
        /// The user's exact approval text for the new master
        #[arg(long)]
        approval: String,
        /// Codex profile for the new agent
        #[arg(long, default_value = "gcm")]
        profile: String,
        /// Existing tmux socket; this command never starts a server
        #[arg(long = "tmux-socket")]
        tmux_socket: std::path::PathBuf,
        /// Existing tmux session name
        #[arg(long = "tmux-session")]
        tmux_session: String,
        /// Retained operation ID for an idempotent retry
        #[arg(long = "op")]
        operation_id: Option<String>,
    },
    /// Cancel one retained bootstrap intent after explicit user confirmation; no process is stopped
    BootstrapCancel {
        #[arg(long = "op")]
        operation_id: String,
        #[arg(long)]
        approval: String,
    },
    /// Record one real target master's CEO handshake
    BootstrapReady {
        #[arg(long = "op")]
        operation_id: String,
        #[arg(long)]
        subject: String,
        #[arg(required = true, trailing_var_arg = true)]
        body: Vec<String>,
    },
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
    /// Set your display name, or restore the current role/pane default
    Name {
        #[arg(required_unless_present = "reset", conflicts_with = "reset")]
        name: Option<String>,
        #[arg(long)]
        reset: bool,
    },
    /// Master creates an ordinary peer with durable native-thread receipts
    Create {
        target_id: String,
        #[arg(long)]
        cwd: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long = "op")]
        operation_id: Option<String>,
    },
    /// Inspect worker status (liveness, identity, agent state, unacked notifications)
    Status {
        /// Optional worker ID to inspect (defaults to all registered workers)
        id: Option<String>,
    },
    /// Read the latest live buffer from an exact registered tmux peer
    Snapshot {
        /// Exact worker ID whose pane buffer is captured
        target_id: String,
        /// Maximum number of recent tmux buffer lines to return (1..=200)
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },
    /// Read one exact peer lifecycle target (defaults to the caller)
    Read {
        /// Exact worker ID to read; omit for the authenticated caller
        target_id: Option<String>,
    },
    /// Change only the selected worker's App Server cwd through the lifecycle owner
    Update {
        /// Exact worker ID whose cwd changes
        target_id: String,
        /// New canonical working directory
        #[arg(long)]
        cwd: String,
        /// Retained operation ID from an earlier attempt
        #[arg(long = "op")]
        operation_id: Option<String>,
    },
    /// Retire one exact peer through the lifecycle owner
    Close {
        /// Exact worker ID to close
        #[arg(
            required_unless_present = "self_current",
            conflicts_with = "self_current"
        )]
        target_id: Option<String>,
        /// Remove this pane's own binding and route; keep its host running
        #[arg(long = "self")]
        self_current: bool,
        /// Why this worker is being closed; recorded for audit
        #[arg(
            long,
            required_unless_present = "self_current",
            conflicts_with = "self_current"
        )]
        reason: Option<String>,
        /// Retained operation ID from an earlier attempt
        #[arg(long = "op", conflicts_with = "self_current")]
        operation_id: Option<String>,
    },
    /// Read one retained lifecycle operation without producing a host effect
    Query {
        /// Retained lifecycle operation ID
        #[arg(long = "op")]
        operation_id: String,
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
    /// Hidden: create .agent-collab skeleton for internal AppSDK initialization
    #[command(hide = true)]
    Init,
    /// User service runtime entrypoint
    #[command(hide = true)]
    Serve,
    /// User maintenance entrypoint
    #[command(hide = true)]
    Up,
    /// User maintenance entrypoint
    #[command(hide = true)]
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
    /// Task board for registered masters, peers, and managed subworkers
    Board {
        #[command(subcommand)]
        cmd: BoardCommand,
    },
    /// Inspect or read messages from durable mailbox
    Mailbox {
        #[command(subcommand)]
        cmd: MailboxCmd,
    },
    /// List registered peers and their local activity projection
    /// (does not report master authority; use `collab master status`)
    Who,
    /// Read-only communication directory for current project (does not require authentication)
    Directory,
    /// List copyable project-qualified delivery addresses with channel and
    /// honest deliverability. Same read-only source as `collab directory`.
    List,
    /// Inspect or explicitly assign collab master authority
    Master {
        #[command(subcommand)]
        command: MasterCmd,
    },
    /// Host-wide CEO coordination across registered project masters
    Ceo {
        #[command(subcommand)]
        command: CeoCmd,
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
    /// Operator inspection and explicit retirement of registered workers
    Worker {
        #[command(subcommand)]
        cmd: WorkerCmd,
    },
    /// Read the latest live buffer from an exact registered tmux peer
    Snapshot {
        /// Exact worker ID whose pane buffer is captured
        target_id: String,
        /// Maximum number of recent tmux buffer lines to return (1..=200)
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },
    /// Current project master clears this project's Collab history and load
    Reset {
        /// Legacy offline-maintenance authorization
        #[arg(long, hide = true)]
        approval: Option<String>,
        /// Confirm that the named control plane may be discarded.
        #[arg(long, hide = true)]
        discard_legacy: bool,
        /// Reset this project's history, routes, assets and runtime load online
        #[arg(long)]
        project: bool,
        /// L3: rebuild the host control plane.
        #[arg(long, hide = true)]
        host: bool,
        /// The live host index root. Required for --host.
        #[arg(long, hide = true)]
        storage_root: Option<std::path::PathBuf>,
        /// L3: also remove ~/.collab/runs/.
        #[arg(long, hide = true)]
        include_runs: bool,
    },
    /// Send a message to another worker
    #[command(alias = "sendmessage")]
    Send {
        /// Recipient address. Optional when the new positional `<address>
        /// --content <正文>` form is used.
        #[arg(long)]
        to: Option<String>,
        /// Explicit anonymous sender; skips authentication and sender validation
        #[arg(long)]
        unknown_sender: bool,
        /// Fixed target session constraint; prevents delivery if recipient changes sessions
        #[arg(long)]
        target_session: Option<String>,
        /// Short topic shown in the notification preview. In the new
        /// `--content` form the daemon fixes a standard notification subject.
        #[arg(long)]
        subject: Option<String>,
        /// New send syntax: plain body of the message. Mutually exclusive with
        /// the legacy positional `<body>` form.
        #[arg(long)]
        content: Option<String>,
        #[arg(long, default_value = "notify")]
        r#type: String,
        #[arg(long)]
        in_reply_to: Option<String>,
        #[arg(long, default_value = "immediate")]
        delivery: String,
        /// Track an obligation: the recipient must reply to this message.
        #[arg(long, conflicts_with_all = ["reply_on_completion", "unknown_sender"])]
        reply_required: bool,
        /// Track an obligation: reply once the delegated work is complete.
        #[arg(long, conflicts_with_all = ["reply_required", "unknown_sender"])]
        reply_on_completion: bool,
        /// Legacy positional body. The new `<address> --content <正文>` form
        /// keeps the address here for the send owner to normalize.
        body: Vec<String>,
    },
    /// Set or inspect the default delivery target for one external conversation
    /// (e.g. a QQ session). Without a trusted conversation context a terminal
    /// cannot mutate the default; use `collab list` to discover addresses.
    Default {
        /// Address form: `<name>` resolves the project CEO entry, `<name.Jenny>`
        /// targets an explicit agent. With no `target` the command prints the
        /// current default for the active conversation context.
        target: Option<String>,
        /// Channel instance that owns this conversation, e.g. `qqbot`. Required
        /// together with `--conversation`; a plain terminal has neither.
        #[arg(long)]
        channel: Option<String>,
        /// Distinct connection of that channel family, e.g. `qqbot@host`.
        /// Defaults to `--channel` for a single-instance channel.
        #[arg(long)]
        channel_instance: Option<String>,
        /// Stable external conversation key, e.g. `<user>:<session>`.
        #[arg(long)]
        conversation: Option<String>,
    },
    /// Discover and explicitly subscribe to finite notifications
    Notify {
        #[command(subcommand)]
        cmd: NotifyCmd,
    },
    /// Owned goal schedule lifecycle only; never controls base notification
    /// reception.
    Goal {
        #[command(subcommand)]
        cmd: GoalCmd,
    },
    /// Block until messages arrive (long-poll)
    Recv {
        #[arg(long, default_value_t = 600)]
        timeout: u64,
        /// Replay one committed receive identity instead of consuming new mail
        #[arg(long = "receive-id")]
        receive_id: Option<String>,
    },
    /// List unread inbox
    Inbox,
    /// Restore current project identity and communication through the existing
    /// user-managed service, then return the authoritative snapshot
    Context {
        /// Set comma-separated work specialties; an empty value clears them.
        #[arg(long, conflicts_with_all = ["query", "return_challenge"])]
        work_roles: Option<String>,
        /// Current master may set another stable name's work specialties.
        #[arg(long, requires = "work_roles", conflicts_with_all = ["query", "return_challenge"])]
        for_name: Option<String>,
        /// Master-only workflow template target: `master`, `peer`, or
        /// `role:<normalized specialty>`.
        #[arg(
            long = "workflow-target",
            conflicts_with_all = ["query", "return_challenge"]
        )]
        workflow_target: Option<String>,
        /// JSON workflow template for --workflow-target; exactly one of this
        /// flag or --reset-workflow-template is required.
        #[arg(
            long = "workflow-template",
            requires = "workflow_target",
            conflicts_with = "reset_workflow_template"
        )]
        workflow_template: Option<String>,
        /// Remove the master override at --workflow-target and restore the
        /// standard identity template.
        #[arg(
            long = "reset-workflow-template",
            requires = "workflow_target",
            conflicts_with = "workflow_template"
        )]
        reset_workflow_template: bool,
        #[arg(long = "op")]
        operation_id: Option<String>,
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        #[arg(long = "app-scope")]
        app_scope: Option<String>,
        #[arg(long = "approve-identity")]
        approve_identity: Option<String>,
        #[arg(long = "approve-grant")]
        approve_grant: Option<String>,
        #[arg(long)]
        provide: Option<String>,
        #[arg(long)]
        query: bool,
        /// Return a registration challenge received in this participant pane.
        #[arg(long, conflicts_with_all = ["query", "provide", "approve_identity", "approve_grant", "recover_communication"])]
        return_challenge: Option<String>,
        /// Restore/verify communication; DSH uses current host facts and loop enrollment.
        #[arg(long, conflicts_with_all = ["query", "approve_identity", "approve_grant"])]
        recover_communication: bool,
    },
    /// Mark messages as read
    Ack {
        ids: Vec<String>,
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
