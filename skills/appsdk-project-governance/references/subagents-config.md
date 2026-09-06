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
appsdk subagent send <id> --subject <topic> "<task>"
appsdk subagent close <id>
```

`start` probes each configured profile once, bounded by timeout, then launches
one Codex session. Reusing an ID returns the existing record, never restarts
it. `starting` is not `idle`: Codex may require trust/auth/approval interaction.
Use status; do not inject automatic confirmation or keep re-sending tasks.
All profiles failing returns a failed record with reasons and no launch.

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
