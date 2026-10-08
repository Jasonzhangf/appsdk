# Resource Coordination and Waits

Before mutating a shared feature, resource, mainline node, gate, integration
lease, install target, or daemon:

1. Refresh durable state and attempt the semantic claim.
2. If occupied, persist the requester as blocked and return holder/responsible
   actor synchronously.
3. Send `RESOURCE_OCCUPIED` only when coordination is needed.
4. Create a bounded wait only for a real blocker/responsible actor.
5. Subscribe to the exact release/deadline event only when async wake is useful.

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
and escalate unresolved work. A managed subagent and an ordinary worker both
escalate to the current Collab master immediately after finding a concrete
solution; they do not wait or dump symptoms. A subagent also copies its
parent when parent is not the master. If no current master exists, escalate to
the task-initiating collaborator. Independent peers may decline a master
collaboration invite. Include the blocking task, responsible actor,
deadline, proposed solution, attempted actions, and requested decision. Never
invent a master from an internal init adapter; master promotion requires
explicit user approval for the exact peer and project plus an authenticated
current binding, and it replaces the recorded holder independent of liveness.
Use `collab context` as the agent's master-authority read. Only the current
master grant holder may delegate; an authenticated peer with explicit approval
may promote or clear the scoped grant. Codex root is not Collab master.
