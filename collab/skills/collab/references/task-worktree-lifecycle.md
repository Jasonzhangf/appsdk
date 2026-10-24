# Task and Worktree Lifecycle

Use this lifecycle automatically for multi-worker tasks once scope and worktree
are known. Bind the semantic feature/resource and owned file paths in the task
scope; check overlapping ownership before shared changes. Registration is not
a prerequisite for unrelated single-worker quality checks.

Engineering delivery and Collab resource close are separate. Verified delivery
can be reported while a worktree is retained with owner and purpose recorded.
Keep the Collab cleanup obligation open until its actual removal requirements
are met; never fake `closed`, remove an unmerged tree, or demand deletion merely
to declare tested work delivered.

```text
latest main
  -> clean declared configured worktree path (<base>/<project-key>/<short-slug>
     when [worktree].base is configured; legacy
     <project-main>/playground/<short-slug> remains valid during transition)
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

Common commands:

```sh
collab context
collab task status [task-id]
collab task conflicts --feature <feature-id>
collab task register <task-id> --feature <feature-id> \
  --worktree <configured-or-legacy-worktree> \
  --branch codex/<short-slug> --base-commit <sha> --priority p2
collab task update <task-id> --status verifying --next "<next evidence gate>"
collab task update <task-id> --status reviewed
collab task deliver <task-id> --evidence "commit=<sha>; gates=pass" \
  --worktree <configured-or-legacy-worktree>
collab task deliver <task-id> --evidence "read-only evidence; gates=pass" \
  --no-candidate
collab task review <task-id> --accept --evidence "review gates=pass"
collab task integrated <task-id> --commit <main-sha> --evidence "main gates=pass"
collab task close <task-id>
```

While a task waits on another peer, inspect that peer with
`collab snapshot <peer-id> --lines <1..200>` before and during the wait. The
bounded buffer shows the current pane text without sending input. Decide from
it whether to keep waiting, escalate, or steer through the normal input path.
The buffer does not prove busy/idle, Codex turn state, message consumption, or
task completion, and it writes no durable receipt.

Closing the last owned task does not cancel this peer's default direct-message
lease. Base reception is daemon-owned and has no subscription-closing command;
end a leftover finite goal schedule with `appsdk goal cancel`, and let
task-owned schedules end with task closure.

- One issue owns one clean worktree. New work must use the configured
  `[worktree].base` when set, for example
  `<base>/<project-key>/<short-slug>`; legacy paths under
  `<project-main>/playground/<short-slug>` remain valid during the transition.
- A worktree is only the task execution directory and has no identity context.
  Run `collab context`. The CLI automatically observes runtime facts, and the
  daemon owns identity selection, creation, recovery, update, registration,
  and binding persistence. Use the same snapshot for this peer's task context,
  peers, and master-authority state. Never create a worktree-local peer, and never
  infer "no master" from a missing worktree directory or a failed `context`.
- Declare task ID, owner, feature/resource ID, worktree, branch, base commit,
  priority, status, and next step before product edits.
- Never share/reuse a worktree. Never depend on dirty main.
- Only the owner may update, deliver, cancel, or normally close. Review and
  exact-main integration require the task owner or current master.
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
or current master uses `task review --accept|--rework` to record the review
decision. Accepted read-only work reaches the typed `no_merge_required`
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
schedule. When a wake arrives, inspect the task, worktree, mailbox, and
wait/block state, then take the next actionable step. Continue execution when
work is available, resolve a blocker when it can, and record/escalate when it
cannot. ACKing a wake is not progress and does not satisfy liveness.

Route escalation by role:

- An ordinary worker reports blockers to the current Collab master immediately.
  Send original failure evidence, attempted remedies and the decision needed;
  do not delay help until root cause or a solution is proven. Apply the main
  Skill's Registered patrol duties for master verification. Independent peers may
  temporarily decline a master collaboration invite to protect their own task.
  If no current master exists, report to the collaborator that initiated the
  task. Legacy managed records grant no new execution authority.
- A peer becomes master only after explicit user approval for that peer and
  project; the caller must hold an authenticated current binding. An approved
  `collab master promote --approval` replaces any recorded holder and is
  independent of the incumbent's or the candidate's liveness. Only the current
  master grant holder may `collab master delegate`. An internal init adapter
  result alone proves neither peer initialization nor master ownership; only
  `collab context` and the returned snapshot have that authority for the
  current agent. A dead recorded thread does not remove the current holder's
  authority. Codex root is not Collab master.
- Master compiles the goal into a dependency graph, then parallel unique-write
  scopes. It publishes each long-horizon task and claims or invites a suitable
  consenting peer only when delivery conditions (done-iff, artifacts, in/out
  of scope, forbidden edits) and test conditions (exact commands, expected
  results, evidence location) are unambiguous. Do not equally split a large
  goal or share a write scope. Bounded execution or observation uses an
  independent `codex exec --profile gcm` worker. Workers execute only the
  approved assignment and return evidence. Report suspected blockers promptly
  with original failure evidence and attempted remedies; do not require a
  proven solution before asking for help. Apply the main Skill's Registered
  patrol duties for master verification and continued peer delivery ownership.

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

Run `collab context` from the current project. The existing global daemon
restores the current session identity and usable binding, route and lease
together. Historical pane owners, old tasks, journals and project socket
records do not gate restoration. User authorization directly replaces the
project master. Agents never start, stop or restart the service.
If worker status, board admission, or a mutation reports a stale binding
generation or `SESSION_THREAD_BINDING_MISMATCH`, preserve the registered
binding and run `collab context` as the only repair entry. Status and board
admission do not repair state. Never manually rebind or infer a generation.
Read task notes separately to resume work; they are not identity proof.
