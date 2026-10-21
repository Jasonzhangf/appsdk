# Legacy managed children and independent GCM workers

This reference records the compatibility boundary for legacy managed-child
configuration and the active execution rule for bounded workers.

## Active execution rule

Long-horizon work belongs to a consenting peer assigned through the public
Collab board. That peer orchestrates bounded execution and observation with
independent `codex exec --profile gcm` workers. Use the documented fallback only
when GCM is unavailable. Do not use Codex built-in spawn/fork/session Agent
slots or managed subagents as the production concurrency path.

One-shot execution and observation go directly to independent GCM workers,
regardless of peer availability. Master and peer each retain their goal's
planning, verification, integration, acceptance and resource closure. Each
registers and manages its own goal; neither substitutes for another goal owner.

Each bounded worker contract states:

- goal and scope;
- allowed and forbidden paths;
- authorized operations;
- resource owner;
- delivery and exact test conditions;
- evidence and report location;
- stop conditions.

## Legacy compatibility boundary

The `[subagent]` configuration tables, `appsdk-subagent` MCP name,
`collab subagent`, and `appsdk subworker` commands are compatibility protocol
tokens. Existing records may be inspected for diagnosis or migration. They
grant no new production execution authority.

Do not start, dispatch, send new work to, bind, or rearm a managed child for a
current goal. Do not convert a refused board invitation or failed GCM launch
into a managed-child fallback. Preserve historical records, tasks, mailboxes,
and cleanup obligations; retire them only through their owning compatibility
flow.

Only `~/.appsdk/config.toml` owns compatibility startup, notification, and
timer policy for records that still consume that configuration. Run
`appsdk config` from the project cwd to validate effective values. Project
overrides stay in `[[projects]]` tables in that same file. Git worktrees inherit
their main project's policy. Do not copy Codex credentials or rewrite profiles.

```toml
[notifications]
enabled = true
mode = "batch"
batch_window_seconds = 60
transport = "appserver"
submit_enter = true
[notifications.events.deadline]
mode = "immediate"
[timers]
enabled = true
tick_interval_ms = 1000
[keepalive]
enabled = true
interval_seconds = 900
max_unacked = 3
[subagent]
profile_priority = ["gcm", "oauth"]
persistent = true
close_on_task_complete = false
[subagent.health]
timeout_seconds = 45
attempts_per_profile = 1
expected_response = "OK"
[subagent.startup]
ready_timeout_seconds = 90
```

Event keys are `direct_message`, `resource_released`, and `deadline`. A fixed
batch window does not slide when new messages arrive. Disabled timers suppress
deadline notification generation, not task timeout safety checks. Configuration
changes affect only newly started compatible records. The global daemon remains
user-managed; agents do not restart it.

## Legacy commands

The compatibility commands are:

```text
appsdk subworker <start|list|status|snapshot|send|close|rearm> ...
collab subagent <start|dispatch|list|status|snapshot|rearm|send|ready|working|close|bind> ...
```

`status` and `snapshot` may preserve historical diagnostic facts. They do not
authorize execution. `snapshot` is transient screen output, not task truth.
`rearm` requires an explicit operator request after exhaustion and never makes
the record valid for new production work.

Existing records and journals must be preserved during upgrade. Do not
downgrade to an older reader after a newer writer appended events. Do not edit
journal, mailbox, route, identity, token, or configuration state by hand.

## Cleanup and recovery

An existing managed record can keep a historical task and cleanup obligation.
Complete or explicitly park that obligation with durable evidence. Do not
delete a dirty, active, shared, or unknown worktree. Extract recoverable commits
and untracked content before any authorized removal.

When upgraded Collab or AppSDK projections change, install through the official
entry and report installed, daemon-loaded, and live behavior separately. An
installed binary alone does not prove the user-managed daemon loaded it.
