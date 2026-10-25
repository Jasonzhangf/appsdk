# Task and Worktree Lifecycle

Use this reference for a concrete Collab task, board action or resource close.
Bind the semantic feature/resource and owned paths before shared changes.
Discussion, status questions and role settings do not require creating a task.
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

Common commands:

```sh
collab task status [task-id]
collab task conflicts --feature <feature-id>
collab task register <task-id> --feature <feature-id> \
  --worktree <owned-worktree> \
  --branch codex/<short-slug> --base-commit <sha> --priority p2
collab task update <task-id> --status verifying --next "<next evidence gate>"
collab task deliver <task-id> --evidence "commit=<sha>; gates=pass" \
  --worktree <owned-worktree>
collab task deliver <task-id> --evidence "read-only evidence; gates=pass" \
  --no-candidate
collab task review <task-id> --accept --evidence "review gates=pass"
collab task integrated <task-id> --commit <main-sha> --evidence "main gates=pass"
collab task close <task-id>
```

While a task waits on another peer, do not require a snapshot before or during
the wait. Use the durable wait contract and the peer's real progress or result
events to continue, escalate, or stay blocked. An on-demand tmux snapshot is
only an optional diagnostic for a tmux peer when current pane text is actually
needed; it does not prove busy/idle, Codex turn state, message consumption, or
task completion, and it writes no durable receipt. See
[resource-waits](resource-waits.md).

Closing the last owned task does not cancel this peer's default direct-message
lease. Base reception is daemon-owned and has no subscription-closing command;
end a leftover finite goal schedule with `appsdk goal cancel`, and let
task-owned schedules end with task closure.

- Use the project/host contract's exclusive external worktree and
  Close-Before-Open lifecycle. A legacy path accepted by the CLI does not
  authorize a new worktree outside that contract. Reuse an existing task tree
  only after its writer has safely handed it back; preserve its valid changes.
- A worktree is only the task execution directory and has no identity context.
  Reuse the current valid context. When recovery is needed, the CLI observes runtime facts and the
  daemon owns identity selection, creation, recovery, update, registration,
  and binding persistence. Use the same snapshot for this peer's task context,
  peers, and master-authority state. Never create a worktree-local peer, and never
  infer "no master" from a missing worktree directory or a failed `context`.
- Declare task ID, owner, feature/resource ID, worktree, branch, base commit,
  priority, status, and next step before product edits.
- Never have concurrent writers in one worktree. Never overwrite dirty main.
- Only the owner may update, deliver, cancel, or normally close. A specified
  reviewer must perform the required review; author/master cannot substitute.
  Unspecified review and exact-main integration retain their existing owner
  and current-master admission rules.
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
reviewer contract; a specified reviewer remains mandatory. Accepted read-only work reaches the typed `no_merge_required`
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

## Work roles and assignment policy

The current master may set another stable name's specialties with
`collab context --for-name '<name>' --work-roles '<complete-list>'` under
existing authorization. Omit roles to preserve them; an explicit empty list
clears them. Role settings do not publish a task or change master authority.

Save task scope, delivery/test conditions and relevant authorization in the
existing board task. Set a reusable work policy only when that default is needed:

```sh
collab board policy --scope project --work-kind '<kind>' --prefer-role '<role>'
collab board policy --scope goal --id '<parent-task>' --expected-revision <observed> --executor '<name>'
collab board policy --scope task --id '<task>' --expected-revision <observed> --reviewer '<name>'
```

Project, goal and explicit task values apply in that order per field. Child
tasks refer to their goal task through `--parent-task`. Defaults do not silently
rewrite already confirmed task constraints. Use `--clear-executor`,
`--clear-reviewer` or `--clear-prefer-role` for an authorized removal; do not
guess clearing semantics from an empty name. Do not create a second role table.

A named executor/reviewer is mandatory; a preferred specialty is a selection
hint. Within authorized selection, master may use
`collab board dispatch '<task>' --expected-revision <observed>` for an existing
task. Missing optional preferences do not require human input. Preserve
explicit user constraints and handle internal scope/resource changes through
the assigning owner and the main Skill's clarification rule.

## Public task board and read-only dashboard

未关闭任务保留；负责人身份的当前绑定失效时，board 负责人显示为空，由 master 重新安排。读取不恢复旧身份、不自动关闭任务，也不修改历史 owner、资源或证据。已关闭任务保留历史负责人。board 是独立业务读取，不参与身份恢复判定。

The task board is the shared work surface for registered masters and peers.
Each active role declares its current work and may queue later work here.
Legacy managed records remain visible only through their existing
compatibility scope; they receive no new assignment. The public view carries
task identity, lifecycle status, owner, and the typed delivery/test conditions.
It never exposes worker tokens, runtime endpoints, role briefs, managed-child
identity, private assignments, or raw journal events. A published task does not
start execution; an invitation does not transfer ownership.

Read the board without consuming messages or changing lifecycle:

```sh
collab board show
```

Any registered execution role publishes a pending task with its contract.
Publishing queues work; it does not start it. Publish several task IDs to queue
later work without consuming execution capacity:

```sh
collab board publish <task-id> --title <t> --description <d> \
  --delivery-condition <d> --test-condition <t> [--priority p0|p1|p2|p3|p4]
collab board claim <task-id> --expected-revision <observed>
```

Claim starts one pending task in the caller's own identity. A peer may claim
its own publication or another authorized task. Read the revision from a current
typed task/board receipt; query `board show` only when that view is missing or
known stale. Claim atomically checks that the task is still pending and that
the caller can start execution; it records the actual executor as owner. A
stale or competing claim fails without taking the task. Do not update a
pending task to `working` to bypass claim. Never claim outside the user's
authorized goal or take another owner's working or blocked task.

For a long-horizon goal, master may instead offer a public task to one ordinary
peer:

```sh
collab board invite <task-id> --to <peer> --expected-revision <observed>
```

Invite and respond are revision-checked. `invite` fails with
`BOARD_STALE_REVISION` when the observed revision is old, and with
`BOARD_PEER_BUSY` when the target already owns an unfinished task or another
invitation. Take the revision from the current typed task view, never from a
message body; an already available valid view avoids another board read.

The invited peer accepts or declines in its own identity. Accept rechecks the
peer's other responsibilities and its live binding before ownership moves:

```sh
collab board respond <task-id> --accept --expected-revision <observed>
collab board respond <task-id> --decline --expected-revision <observed> --reason "<text>"
```

Decline requires a nonempty reason and leaves publisher ownership unchanged. The
owner updates progress on its own task with a current revision:

```sh
collab board update <task-id> --expected-revision <observed> --status <s> [--next <step>]
```

The master withdraws an unaccepted invitation with its observed revision and a
reason; withdrawal never reclaims peer-owned execution:

```sh
collab board withdraw <task-id> --expected-revision <observed> --reason "<text>"
```

Public work uses claim or the board invitation above. `collab subagent
dispatch` is a legacy private managed path and grants no new production work.
If a peer declines a long-horizon invitation, retain that goal and arrange
another consenting peer or decompose it under the current orchestrator.
Dispatch its bounded execution or observation directly to independent
`codex exec --profile gcm` workers. A refusal does not authorize the legacy
private managed path or transfer orchestration responsibility to a subworker.

`collab dashboard` opens a read-only loopback Web observer for the same public
board. It requires a registered identity, does not bootstrap or restart the
daemon, and never consumes messages:

```sh
collab dashboard [--port <loopback-port>]
```

It prints one JSON line and then serves until stopped:

```json
{"url":"http://127.0.0.1:<port>/#<capability>","read_only":true,"refresh_seconds":2}
```

Open that printed URL in a browser on the same host. The capability is a fresh
per-run read credential carried only in the URL fragment: the page keeps it in
memory, sends it as a bearer header, and never puts it in a query string or a
log. It grants no actor identity and no write authority. The page re-reads the
board projection every 2 seconds; it is a polling view, not an event stream.

Any registered identity on that host may start one, so a master or an ordinary
peer can expose the project board for the human controller. It is an observer,
not a control surface: it cannot publish, invite, respond, update, withdraw or
close anything.

The observer rejects a request whose `Host` is not its own bound authority with
`DASHBOARD_HOST_REJECTED` (403) and a request without the exact capability with
`DASHBOARD_CAPABILITY_REQUIRED` (401). While the daemon is unreachable the
board endpoint answers 503 with the exact daemon error instead of an empty
board. Static assets carry no state and no token.

Legacy managed identities and private child tasks remain compatibility facts.
They do not receive new public assignments. Preserve their historical privacy
and visibility semantics; do not create or bind another child from current
work. New bounded execution or observation uses an independent GCM worker.

An owner edits the descriptive part of its own independent task without
changing lifecycle. Every flag is required and the revision is checked:

```sh
collab board describe <task-id> --expected-revision <observed> --title <t> \
  --description <d> --delivery-condition <d> --test-condition <t>
```

`collab-mcp` exposes the same operations as `collab_board_show`,
`collab_board_publish`, `collab_board_claim`, `collab_board_invite`, `collab_board_respond`,
`collab_board_withdraw`, `collab_board_update`, `collab_board_describe`, and
`collab_task_decline`. Each takes `expected_revision` as an integer of at least
1; `collab_task_decline` takes `legacy_assignment: true` only for an unstarted
legacy assignment.

When any board command fails, the CLI preserves the server's full typed
response, including durable IDs and `repair_required`. Read the `collab
response:` line before retrying: a durable outcome is not a retryable failure.

## Task liveness and escalation

The owner retains the task through required delivery and resource closure.
Act on the current authorized next action or a relevant result/wake. Do not
poll every task, worktree and mailbox after an unrelated message. Use the
main Skill's [clarification rule](../SKILL.md#澄清决策与阻塞) for help and
[resource-waits](resource-waits.md) for a real dependency. Unchanged waiting
does not require repeated reports, snapshots or a new task.

Record actual affected state and preserve claims; asking for help does not
transfer ownership. A master verifies a formal blocker and arranges help or
authorized reassignment. A known technical question can go directly to its
owner. Never force-close unfinished work to silence reminders. Use
[notifications](notifications.md#patrol-duties-and-finite-schedules) for a
needed finite wake, preserving base direct-message reception.

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
