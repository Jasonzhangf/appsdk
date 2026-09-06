# Persistent subagents and shared policy

Only `~/.appsdk/config.toml` owns startup/notification/timer policy. Run
`appsdk config` from the project cwd to validate and inspect effective values.
Global defaults work when the file is absent; project overrides stay in
`[[projects]]` tables in that same file. Git worktrees inherit their main
project's policy. Never copy Codex credentials or rewrite its profiles.

```toml
[notifications]
enabled = true
mode = "batch" # immediate or batch
batch_window_seconds = 60
transport = "tmux"
submit_enter = true
[notifications.events.deadline]
mode = "immediate"
[timers]
enabled = true
tick_interval_ms = 1000
[keepalive]
enabled = true
interval_seconds = 900 # minimum 15 minutes
max_unacked = 3 # hard maximum, not an unlimited retry setting
[subagent]
profile_priority = ["gcm", "oauth"]
persistent = true
close_on_task_complete = false
[subagent.profiles.gcm]
codex_profile = "gcm"
[subagent.profiles.oauth]
codex_profile = "oauth"
model = "gpt-5.6-luna"
[subagent.health]
timeout_seconds = 45
attempts_per_profile = 1
expected_response = "OK"
[subagent.startup]
ready_timeout_seconds = 90
[subagent.tmux]
name_template = "{cwd_name}-subagent-{short_id}"
# Optional:
# [[projects]]
# root = "/absolute/project/root"
# [projects.notifications]
# mode = "immediate"
```

Event keys: `direct_message`, `resource_released`, `async_result`, `deadline`;
each accepts `mode = "inherit" | "immediate" | "batch"`. A fixed batch window
does not slide when new messages arrive. Disabled timers suppress deadline
notification generation, not task timeout safety checks. New subagents read
config at creation; existing daemons read policy at startup. Use controlled
daemon restart after a policy change. Existing tasks/mailboxes remain intact.

From a registered tmux parent:

```text
appsdk subagent start --id <unique-request-id>
appsdk subagent list
appsdk subagent status <id>
appsdk subagent snapshot <id> --lines 40
appsdk subagent send <id> --subject <topic> "<task>"
appsdk subagent close <id>
```

`start` probes each configured profile once, bounded by timeout, then launches
one Codex session. Reusing an ID returns the existing record, never restarts
it. `starting` is not `idle`: Codex may require trust/auth/approval interaction.
Use status; do not inject automatic confirmation or keep re-sending tasks.
All profiles failing returns a failed record with reasons and no launch.

An already trusted cwd and healthy authenticated profile should start without
new trust/auth interaction. Repeated prompts on that path are a startup bug,
not an instruction to reinitialize credentials or accept prompts automatically.

## Finite task keepalive and observation

`send` creates the canonical `task-<message-id>` task in the Collab task list.
Child `working` claims it; bind code work with `collab_task_relocate`, not a
duplicate task registration. Complete the real task lifecycle; `ready` changes
session availability only and never marks unfinished tasks complete.

Unfinished actionable tasks are grouped by worker. Explicit idle for 15 minutes
allows one activation; ACK it once with `collab ack <notification-id>` (MCP:
`collab_ack`), then work or record the real blocker. Messages sent by the worker,
fresh ACKs and positive working observations count as activity. Unknown remains
unknown; absent/unknown/working receive no activation. Blocked/waiting tasks
follow their declared wait, not this continuation path.

Three consecutive unconfirmed attempts exhaust the durable budget. After the
third response window, status marks `suspected_offline`; it does not claim the
process is dead. Failed/uncertain sends count, restart does not reset the budget,
and no process is respawned. Only an explicit parent/operator request may use
`appsdk subagent rearm <id>`; never rearm automatically to bypass exhaustion.

`status` returns observed state, task list, parent mailbox, keepalive counters
and notification/ACK history. It never captures the screen. `snapshot` returns
only the requested last 1..200 tmux lines and capture time, never sends a notice
or interprets screen text as task/control truth. Snapshot may contain sensitive
terminal output: request only when relevant, do not republish it by default.

A non-tmux agent may use local project `list/status/snapshot` without registering
a fake peer. Initialization and queries explicitly report `notification_channel:
none`: no push channel exists for that observer. Check the mailbox in `status`
yourself; do not wait for an automatic completion notification. This is not a
quality gate or a reason to stop independent work. Mutating parent operations
retain authenticated ownership checks.

The child uses the injected `appsdk-subagent` MCP: `collab_init`, then
`collab_subagent` with `action=ready, id=<id>`. The launcher forwards the live
tmux binding automatically; do not ask the user to set environment variables.
Use MCP, not sandboxed shell registration. Only its bound identity/pane may
report ready. It accepts a dispatched task with `action=working`, manages its
own task/worktree lifecycle, sends results to the parent, and calls `ready`
when done. Repeated idle reports are no-ops. Remain idle, not an ACK/poll loop.
Task progress stays in Collab task records; no second task queue exists here.

Only the creating parent may send/close; a user-requested early close may
interrupt work, but never deletes worktrees or marks tasks complete. Closing
uses exact session/pane identity. It does not stop the project daemon.
Daemon restart replays records without automatically respawning or redispatching.
If startup was interrupted, preserve the record/session and inspect status;
do not reuse its ID to create another process.

Upgrade: install reviewed Collab and AppSDK releases, then `collab down` /
`collab up` per already-running project. No migration/reset/init of old tasks
is required. Preserve journals and running subagent sessions. Do not downgrade
to an older reader after new subagent events have been written to the journal.
