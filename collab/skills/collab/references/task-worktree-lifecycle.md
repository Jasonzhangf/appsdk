# Task and Worktree Lifecycle

Use this reference for an existing Collab task or concrete resource action.
Bind the semantic feature/resource and owned paths before shared changes.
Discussion, status queries and role settings do not require a new task.
Registration is not a prerequisite for independent quality checks.

Engineering delivery and Collab resource close are separate. Verified delivery
can be reported while a worktree is retained with owner and purpose recorded.
Keep the Collab cleanup obligation open until its actual removal requirements
are met; never fake `closed`, remove an unmerged tree, or demand deletion merely
to declare tested work delivered.

```text
latest main
  -> exclusive worktree at the project/host-contract path
  -> register owned task
  -> implement and verify
  -> task deliver {code candidate | --no-candidate read-only evidence}
  -> task review --accept
code -> sync latest main and verify exact delivered candidate
     -> acquire short integration/resource lease
     -> integrate, verify main, push main
     -> task integrated --commit <main-sha> --evidence <main-gates>
read-only -> typed no_merge_required
either -> owned-resource cleanup
  -> cleanup_pending
  -> cleanup_verified receipt
  -> closed
```

States:

```text
working -> verifying -> reviewed -> delivered -> accepted -> merged -> closed
delivered(read-only) -> accepted -> cleanup_pending -> closed
blocked -> waiting -> blocked|working
delivered -> rework -> working
accepted -> rework -> working
* -> cancelled
```

Select the command for the actual transition from the
[operations guide](../operations.md#common-command-card); this lifecycle is not
a fixed command sequence. Reuse valid task and receipt facts.
For a real resource dependency, use [resource waits](resource-waits.md).
Pane snapshots are optional diagnostics, not a prerequisite before or during waits.

Closing the last owned task does not cancel this peer's default direct-message
lease. Base reception is daemon-owned and has no subscription-closing command;
end a leftover finite goal schedule with `appsdk goal cancel`, and let
task-owned schedules end with task closure.

- Use the project/host contract's exclusive worktree and Close-Before-Open
  lifecycle. CLI acceptance of a legacy path does not authorize a new tree there.
- A worktree is only the task execution directory and has no identity context.
  Reuse valid context; run `collab context` when recovery is needed. The CLI observes runtime facts, and the
  daemon owns identity selection, creation, recovery, update, registration,
  and binding persistence. Use the same snapshot for this peer's task context,
  peers, and master-authority state. Never create a worktree-local peer, and never
  infer "no master" from a missing worktree directory or a failed `context`.
- Declare task ID, owner, feature/resource ID, worktree, branch, base commit,
  priority, status, and next step before product edits.
- Never share a write scope or reuse another owner's worktree. The same owner
  may continue the same task in its existing tree. Never depend on dirty main.
- Only the owner may update, deliver, cancel, or normally close. A named
  reviewer must perform the required review; author/master cannot substitute.
  Other review and integration admission follow the current task contract.
- Force close exceptions are explicit and audited: the current master may close
  any task; with no current master the owner may self-close, or a registered peer
  may close a task whose owner registered transport identity is lost.
- A bound worktree creates a mandatory cleanup obligation. Keep its exact path
  and branch bound until close.
- Before any removal, extract recoverable commits and untracked content that
  the task still needs. Dirty, active, shared, or unknown worktrees are
  PROTECTED_HOLD. Preserve and report them; never force-remove them.
- `delivered`/`merged` are not cleanup. Close requires a clean, removed,
  verified-absent worktree plus durable cleanup receipt.
- Close rejects unmerged commits, dirty paths, path escape, wrong branch,
  unique unmerged history, or cleanup failure.
- Cancellation cannot bypass cleanup. Dirty/missing/unproven paths are never
  force-removed. Terminal state without a receipt is an audit failure.

Delivery, review, and applicable integration are separate durable milestones.
`task deliver` records evidence and an explicit delivery kind. Code delivery
uses `--worktree`; the exact delivered candidate must resolve before
integration. Explicit read-only work uses `--no-candidate`; a missing or
unresolved code candidate never becomes read-only by inference. The task owner
or current master uses `task review --accept|--rework` under the applicable
reviewer contract; a named reviewer remains mandatory. Accepted read-only work reaches the typed `no_merge_required`
disposition and has no integration milestone. For code delivery, `task
integrated` accepts a commit reachable from `refs/heads/main` and records
mainline evidence, but the exact delivered candidate must also be reachable
there. Direct status updates cannot bypass review or integration for current
lifecycle records. A persisted pre-review `accepted` task with no lifecycle
evidence retains an owner-local `accepted→merged` compatibility path; it
records task state only and does not create review or integration evidence.

For accepted code delivery, `task review --accept` also registers the
applicable daemon-owned pending merge keyed by task id and notifies the current
master. That registration, not a chat message, is the single source for the
merge obligation: the master stays busy, changes session, or restarts, and the
obligation survives. Explicit read-only acceptance creates no pending merge and
no merge wake. While an applicable pending merge exists:

- `collab context` for the current master lists a `merge_pending` operation.
- `appsdk longhorizon show` prints a `待合并` section and includes
  `pending_merges` in `--json`.
- master idle wake text and the idle batch body include `pending_merges`.

The master resolves an applicable code obligation only by moving the accepted candidate onto
`refs/heads/main` and recording `collab task integrated --commit <sha>
--evidence "<main gates>"`. That resolves the pending merge in the same
transaction. `accepted -> rework`, cancellation, or an authorized force close
also resolves it. `collab task close` on a still-pending code merge fails with
`TASK_MERGE_PENDING`; the master cannot lose the merge by staying busy, and the
task cannot close before the merge is recorded.

While the pending merge exists, only the current master may record `task
integrated`; an owner that tries to self-integrate gets `TASK_MERGE_PENDING`.
The obligation is registered only when a current master exists, so a master-less
project keeps the plain owner self-integration lifecycle while still requiring
the exact delivered candidate to be reachable from main.

`task integrated` on explicit read-only work returns
`TASK_NO_MERGE_REQUIRED`. A code delivery whose candidate cannot be resolved
returns `TASK_DELIVERY_CANDIDATE_UNRESOLVED` at integration or normal close;
rework and redeliver it with the correct code intent. Once review and
applicable integration are complete, the owner removes only its clean owned
worktree, records the cleanup receipt, and closes normally.

## Task liveness and escalation

An assigned task remains live until its actual cleanup receipt and `closed`
state exist. The owner acts on an explicit dispatch, a bounded direct-message
lease, a supported timer/wake, or a real external event; it does not poll on a
schedule. On a relevant wake, refresh missing or stale facts for the affected
task and take its next actionable step; do not query every surface. Continue execution when
work is available, resolve a blocker when it can, and record/escalate when it
cannot. ACKing a wake is not progress and does not satisfy liveness.

Route escalation by role:

- An ordinary peer sends missing facts to their owner, design questions to the
  designer and scope/resource blockers to the current Collab master.
  Send original failure evidence, attempted remedies and the decision needed;
  do not delay help until root cause or a solution is proven. Apply the main
  Skill's Registered patrol duties for master verification. Independent peers may
  temporarily decline a master collaboration invite to protect their own task.
  If no current master exists, report to the collaborator that initiated the
  task. Legacy managed records grant no new execution authority.
- Master authority follows [migration-daemon](migration-daemon.md#master-and-deprecated-interfaces).
  Goal routing and patrol responsibilities follow the [main Skill](../SKILL.md#task-routing-and-role-responsibilities).
  An unanswered internal request does not become a human approval gate.
  Ask humans only for exclusive facts, uncovered authority or user-constraint changes.

For every escalation include `task_id`, current durable status, exact blocker or
wait target, failure evidence, attempted resolution/results, and the decision/action
needed. Include a cause or proposed solution when known; otherwise mark it
unknown and request help promptly. Keep the task blocked or waiting truth durable; do not create a
duplicate task, silently release a claim, or wait without a next check and
escalation path.

Before a patrol cycle ends, preserve and report PROTECTED_HOLD resources. If
continued scheduling is required, explicitly re-arm a consumed one-shot goal
or deadline wake. A read, ACK, summary, or dispatch receipt is not completion.

## Lifecycle recovery

Use [stable-name recovery](stable-name-recovery.md) for current identity,
binding and retained work; this reference does not define another recovery protocol.
If worker status, board admission, or a mutation reports a stale binding
generation or `SESSION_THREAD_BINDING_MISMATCH`, preserve the registered
binding and run `collab context` as the only repair entry. Status and board
admission do not repair state. Never manually rebind or infer a generation.
Read task notes separately to resume work; they are not identity proof.
