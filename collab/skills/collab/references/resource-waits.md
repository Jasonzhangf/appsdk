# Resource Coordination and Waits

Before mutating a shared feature, resource, mainline node, gate, integration
lease, install target, or daemon:

1. Refresh durable state and attempt the semantic claim.
2. If occupied, persist the requester as blocked and return holder/responsible
   actor synchronously.
3. Send `RESOURCE_OCCUPIED` only when coordination is needed.
4. Create a bounded wait only for a real blocker/responsible actor.
5. Subscribe to the exact release/deadline event only when async wake is useful.

## On-demand tmux observation while waiting

Do not require a snapshot before creating a wait or on each wake. Snapshot is an
optional, on-demand tmux diagnostic, not a completion proof or a required
pre-probe. Use it only when the target is a tmux peer and you actually need
current pane text to decide how to proceed:

```sh
collab snapshot <peer-id> --lines 40
```

- The snapshot is the latest tmux pane buffer only.
- It proves no busy/idle state, Codex turn state, message consumption, or task
  completion.
- It sends no input, consumes no mail, bumps no revision, and writes no
  durable receipt.
- Use the buffer only to decide: keep waiting, escalate, or steer through the
  normal contractual input path.
- If the target is an AppServer or DSH peer, snapshot is unsupported. Keep the
  existing wait contract and rely on the peer's real progress or result events;
  do not invent a substitute state claim or call snapshot unconditionally.

```sh
collab sendmessage --to <peer> --subject resource-busy "RESOURCE_OCCUPIED ..."
collab sendmessage --to <peer> --subject resource-free "RESOURCE_RELEASED ..."
collab sendmessage --to <peer> --subject result-ready "The result is ready; query the mailbox."
```

Every wait records waiter, exact blocking task, responsible actor,
`resource_conflict`, finite deadline, resume events, and escalation path.

The server rejects self-wait, missing owner, unrelated resource, terminal or
delivered waits, missing deadline/resume path, and direct/transitive cycles.
Timeout makes the waiter explicitly blocked and never sends a message or
releases a claim. Holder close clears obsolete wait edges and creates a
`RESOURCE_RELEASED` notification only for an exact matching subscription.

Waiting is not abandonment. On each supported timer/wake or direct wake, the
waiter must re-read the durable conflict, try any locally available resolution,
and escalate unresolved work. The current master is the default receiver for a
blocked resource; when the open question is instead a task-fact or
implementation question, route it to the responsible owner peer, and when it is
an architecture or design trade-off, route it to the chief designer, choosing
the receiver by question type per
`coordination_rules.clarification_route_rule` in
`references/workflow-templates.json`. Escalate immediately with the evidence
already held; a concrete solution or proven root cause is not required before
asking for help. Do not wait for a fix to appear, and do not dump un-evidenced
symptoms. If no current master exists, escalate to the task-initiating
collaborator. Independent peers may decline a master collaboration invite.
Include the blocking task,
responsible actor, deadline, attempted actions, requested decision, exact
failing input/error or evidence path, and, when an external dependency blocks
continuation, that dependency and the resume condition. Continue any unaffected
work meanwhile. Never invent a master from an internal init adapter; master
promotion requires explicit user approval for the exact peer and project plus
an authenticated current binding, and it replaces the recorded holder
independent of liveness.
Use `collab context` as the agent's master-authority read. Only the current
master grant holder may delegate; an authenticated peer with explicit approval
may promote or clear the scoped grant. Codex root is not Collab master.

Dirty, active, shared, or unknown resources remain PROTECTED_HOLD. Do not
remove them or terminate their owner's process. If the wait cycle follows a
consumed one-shot goal or deadline wake and scheduling must continue, re-arm
that wake explicitly before ending the cycle.
