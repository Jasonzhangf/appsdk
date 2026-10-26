# Verification

Read this for Collab source changes, release, install, or live protocol
verification—not ordinary command use.

Every official release compile uses `scripts/build-collab.sh`, which increments
the host-global `~/.collab/build-version` counter under one lock; there is no
manual patch bump. Direct release builds fail with an instruction to use that
entry. Use `scripts/install-global-collab.sh` so the release candidate is built
once and that candidate is installed. Verify the affected behavior, release
build version, installed versions and canonical command paths. Refresh the Skill with the
exact `$CARGO_HOME/bin/collab` binary, not a bare command that may resolve an
older PATH entry. Remove only legacy copies whose ownership is established, then
validate the affected current behavior: do not
migrate or replay old local control-plane history. The global daemon is managed
by the user. Agents do not operate its lifecycle or request maintenance credentials.
Installation and live activation are separate results. After user activation,
verify the affected installed entry and live behavior. MCP initialization is
needed only for an affected MCP surface, not every installation or recovery.

Before milestone review, select checks for changed behavior and concrete failure
modes. The following are coverage options, not a mandatory sequence. Pure
documentation uses link, syntax and diff checks; unrelated changes do not build,
install or replay all transports:

- architecture/resource/function/verification gates;
- format, unit/state-machine tests, `scripts/build-collab.sh`;
- explicit `task deliver --no-candidate` reaches reviewed `no_merge_required`,
  creates no pending merge or merge wake, and can close after owned cleanup;
- unresolved code delivery stays code, and accepted code cannot integrate or
  close until the exact delivered candidate is reachable from `refs/heads/main`;
- stale runtime generations reject with no durable side effects, ordinary
  `collab context` repairs the worker/binding projection, and context, worker
  status, and board admission then agree;
- isolated two-peer AppServer blackbox in a disposable project: sender persists
  a message, native RPC starts/steers/queues the recipient thread, receiver runs
  `collab recv`, and sender observes the durable consumption receipt. Never
  inject a test notice into an existing production project or Agent conversation;
- restored peer and restored master each complete send, receive and reply
  through the current route; durable send acceptance and role assignment alone
  do not prove message consumption;
- retain the isolated tmux-only compatibility e2e for peers with no AppServer
  candidate; it cannot serve as fallback evidence for an AppServer-bound peer;
- applicable migration replay with durable-state preservation;
- duplicate-daemon rejection without PID/socket corruption;
- no wake for shell/absent/unknown; working Agents receive a due batch;
- explicit messages remain durable without active subscription;
- subscriptions preserve their owner, exact-subject and finite occurrence
  semantics from [notifications](notifications.md); reusable base reception
  and standing master patrol are not finite task schedules;
- pending eligible messages coalesce under the bound event policy's batch window;
- failed wake and daemon restart never replay an attempted batch;
- one ID/subject/original-body delivery preserves a reusable direct-message
  lease and records one accepted native RPC; RPC acceptance proves only
  transport submission, while `collab recv` and its durable receipt prove
  consumption;
- successful resource/deadline delivery consumes only its matching occurrence;
  repeated finite deadlines preserve their remaining count;
- release clears obsolete wait state; the daemon re-arms a registered identity's
  default direct-message lease from the current selected binding;
- board-idle patrol follows [its canonical contract](board-idle-patrol.md),
  including fresh idle observation, periodic fallback, task-scoped cancellation
  and standing master patrol; no additional synthetic polling, inferred waiting, progress/ACK
  loop, implicit first-register master, treating Codex root as Collab
  master, legacy managed production dispatch, heartbeat, or `/goal` semantics; skill-level owner checks
  run only on supported timer/wake or direct wake and then continue or escalate
  real unfinished tasks; explicit user-approved `master promote` replaces the
  current holder independent of liveness, only the current master grant holder
  may delegate, and an authenticated peer with explicit approval may clear the
  scoped grant; independent peers may decline a master invite. Ordinary
  long-horizon work uses board publish/claim/invite; bounded execution and
  observation uses independent `codex exec --profile gcm`. Legacy managed
  records grant no new production execution authority; master-only rules
  require an explicitly authorized master grant.

Review the complete milestone at the host contract's integration boundary after
applicable author checks and public-entry evidence pass. Findings need affected
verification and finding review; unchanged valid evidence is reused. Integration
and cleanup follow the delivery owner. Install and replay only when that surface
is in scope, after user-managed activation where needed. Source, installation
and live results remain separate.
