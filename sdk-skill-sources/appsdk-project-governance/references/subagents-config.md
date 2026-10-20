# Legacy managed-subworker compatibility

This reference is compatibility guidance only. New work must not use a managed
subagent as the execution or observation path.

The production concurrency contract is owned by the installed Collab Skill:

- ordinary peer assignment uses public board publish/invite;
- long-horizon work goes to a peer first;
- that peer orchestrates independent `codex exec --profile gcm` workers;
- master starts a GCM worker only when every eligible peer is busy;
- the documented fallback applies only when GCM is unavailable.

Do not use Codex built-in spawn/fork/session Agent slots as a substitute.

## Legacy record handling

The `[subagent]` table in `~/.appsdk/config.toml`, `appsdk-subagent` MCP name,
`appsdk subagent`, `appsdk subworker`, and `collab subagent` actions are retained
only to inspect, migrate, or retire existing managed records. Existing legacy
records may still contain startup, keepalive, and notification counters.
Preserve those records until their owner resolves or migrates them.

For a legacy record:

1. Read the exact status and current task before any mutation.
2. Preserve its journal, mailbox, worktree, and evidence ownership.
3. Use an explicit owner-scoped close or migration path when the record is
   terminal.
4. Do not rearm an exhausted record to bypass its budget.
5. Do not copy credentials or edit routes, journals, mailboxes, or identity
   files by hand.

The presence of a legacy record does not grant an execution role. It does not
make the caller a Collab peer, master, or ordinary board assignee. Use
`collab context` for current registered identity and the public board for
ordinary assignment.
