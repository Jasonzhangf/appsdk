# Resource Coordination and Waits

Use this only for an actual shared-resource dependency. Apply the authoritative
[Table B](../operations.md#table-b-resource-waits-block-approval-and-queue-continuation)
for resource wait, business-block approval and queue continuation branches.
Before a conflicting
write to a feature, resource, gate, integration lease or install target:

1. Refresh durable state and attempt the semantic claim.
2. If a real resource conflict names a holder task, record a resource wait with
   `collab task wait '<id>' --for '<holder-id>'`. Read the returned wait state.
   A resource wait is distinct from a business-block request.
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
releases a claim. It does not approve a business-block request or establish
that an execution slot is available. Holder close clears obsolete wait edges and creates a
`RESOURCE_RELEASED` notification only for an exact matching subscription.

After a resource wait or an approved business block is recorded, run
`collab board show` and select only currently eligible authorized work.
Claim/start a queued candidate only when current admission permits it. If the
candidate conflicts with resources retained by the waiting task, leave it
queued and select another nonconflicting candidate. Do not release resources
implicitly. On a release or recovery notice, run `collab task status '<id>'`
and `collab board show`; verify the actual blocker and capacity before using
the existing resume transition. See the [patrol decision table](board-idle-patrol.md#table-c-patrol-wakes-and-cancellation)
for task-scoped cancellation when only an evidenced external dependency remains.

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
remove them or terminate their owner's process. For a formal same-task handoff,
follow [same-task transfer and late artifacts](task-worktree-lifecycle.md#same-task-transfer-and-late-artifacts).
If the wait cycle follows a
consumed one-shot goal or deadline wake and scheduling must continue, re-arm
that wake explicitly before ending the cycle.
