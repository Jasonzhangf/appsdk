# Resource Coordination and Waits

Use this only for an actual shared-resource dependency. Before a conflicting
write to a feature, resource, gate, integration lease or install target:

1. Refresh durable state and attempt the semantic claim.
2. If occupied, persist the requester as blocked and return holder/responsible
   actor synchronously.
3. Send `RESOURCE_OCCUPIED` only when coordination is needed.
4. Create a bounded wait only for a real blocker/responsible actor.
5. Subscribe to the exact release/deadline event only when async wake is useful.

## Bounded observation while waiting

Use current durable wait and task evidence first. If a concrete diagnostic gap
requires pane text, inspect a supported peer with a bounded snapshot:

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
  existing wait contract and do not invent a substitute state claim.

Use the actual result or release event to resume. Do not send a second message
merely to tell the receiver to query a result it already received.

Every wait records waiter, exact blocking task, responsible actor,
`resource_conflict`, finite deadline, resume events, and escalation path.

The server rejects self-wait, missing owner, unrelated resource, terminal or
delivered waits, missing deadline/resume path, and direct/transitive cycles.
Timeout makes the waiter explicitly blocked and never sends a message or
releases a claim. Holder close clears obsolete wait edges and creates a
`RESOURCE_RELEASED` notification only for an exact matching subscription.

Waiting is not abandonment. On a supported wake, use current conflict evidence,
try an authorized resolution and request help when needed. Facts go to the
owning peer, design interpretation to the designer, and scope/resource conflicts
to the master or task initiator when no master exists. Send the original failure,
attempted remedies, affected dependency and needed decision. A proven solution
is not a prerequisite for help. Continue independent work; an unanswered
internal request is not a reason to ask the human. Ask the human only for an
exclusive fact, uncovered authorization or changed user constraint.
Master authority and recovery follow [migration-daemon](migration-daemon.md#master-and-deprecated-interfaces).

Dirty, active, shared, or unknown resources remain PROTECTED_HOLD. Do not
remove them or terminate their owner's process. If the wait cycle follows a
consumed one-shot goal or deadline wake and scheduling must continue, re-arm
that wake explicitly before ending the cycle.
