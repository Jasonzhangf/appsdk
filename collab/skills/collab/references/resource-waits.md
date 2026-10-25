# Resource Coordination and Waits

Use this reference for an actual shared-resource conflict. Ordinary queries,
messages and independent work do not need a claim or wait ceremony.

Before a conflicting write, use the resource owner's current claim/admission
result. If occupied, preserve the holder, affected task and original refusal.
Coordinate with the responsible owner; record a bounded wait only when that
dependency prevents the next action. Subscribe to the exact release/deadline
event only when an asynchronous wake is useful. A claim failure does not grant
permission to mutate the resource or its owner's process.

Every real wait records waiter, blocking task, responsible actor,
`resource_conflict`, deadline, resume event and escalation path. The server
rejects self-wait, missing owner, unrelated resource, terminal/delivered waits,
missing deadline/resume path and cycles. Timeout does not release another
owner's claim. Holder close clears obsolete wait edges and creates the exact
matching subscribed `RESOURCE_RELEASED` notification.

Use the existing wake/result to resume. Refresh only facts needed for the
next affected action; do not query every status surface on each wake or send
another message merely to say that a result is available elsewhere.

## Help and continued work

Choose the receiver through the main Skill's [clarification rule](../SKILL.md#澄清决策与阻塞).
Provide the concrete question, current evidence, affected work and requested
decision; include the dependency and resume condition for a real blocker.
Unknown cause stays unknown. Root-cause proof or a proposed fix is not required
before asking for help. Formal resource/scope arbitration stays with the
current master; a technical question does not automatically require arbitration.
Continue unaffected authorized work while waiting. No reply does not authorize
resource takeover, polling, or shifting routine coordination to the human.

## Optional observation

Only when current pane text is needed for diagnosis of a tmux peer:

```sh
collab snapshot <peer-id> --lines 40
```

Snapshot is an observation, not busy/idle, consumption or completion proof.
It is unsupported for AppServer/DSH targets; use their real result events.
Do not require snapshot before waiting or create another pre-probe.

Preserve dirty, active, shared or unknown resources as PROTECTED_HOLD.
Master authority and self-restoration follow the main Skill, not a wait exception.
Finite schedule management stays in [notifications](notifications.md#patrol-duties-and-finite-schedules).
