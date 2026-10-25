# Verification

Read this for Collab source changes, release, install, or live protocol
verification—not ordinary command use.

Every official release compile uses `scripts/build-collab.sh`, which increments
the host-global `~/.collab/build-version` counter under one lock; there is no
manual patch bump. Direct release builds fail with an instruction to use that
entry. Use `scripts/install-global-collab.sh` so the release candidate is built
once and the exact candidate bytes are installed. Verify the semantic source
baseline, release build version, installed binary versions, canonical command
paths, and refreshed embedded Skill byte-for-byte. Refresh the Skill with the
exact `$CARGO_HOME/bin/collab` binary, not a bare command that may resolve an
older PATH entry. Remove only legacy copies whose binary identity proves they
are matching Collab artifacts, then validate the new baseline only: do not
migrate or replay old local control-plane history. The global daemon is managed
by the user. Agents do not operate its lifecycle or request maintenance credentials.
Installation and live activation are separate results. After user activation,
verify `collab context`, MCP initialize, identity, durable state and live replay.

The canonical candidate delivery sequence is a single source, single sink DAG.
Every applicable node must finish before claiming delivery:

```text
latest_main_candidate
  -> targeted_tests + build/install
  -> user-managed runtime activation when applicable (outside the agent interface)
  -> installed binary + collab context + affected bridge service live checks
  -> black-box live behavior replay + author debug complete
  -> applicable independent architecture review PASS
  -> commit/merge/push and cleanup
```

Before changing the installed binary, inspect the current source version,
installed version, canonical paths, and daemon PID/socket:

```sh
cargo metadata --no-deps --format-version 1
collab --version
command -v collab
command -v collab-mcp
collab status --all
```

If the version or command path is stale after installation, fix PATH or refresh
the shell command cache (`rehash` in zsh, `hash -r` in bash), then verify that
`command -v collab` and `command -v collab-mcp` resolve to the exact
`$CARGO_HOME/bin` pair. Do not hand-copy binaries, leave a second managed entry,
or select an older binary as a fallback.

The manifest `verification_gates` list is a candidate menu, not a universal
checklist. Select checks by the affected behavior using the project appsdk-dev
risk-selection owner: a prompt or documentation edit does not require full
build, migration replay, or live replay; a delivery, transport, or installation
change still requires its real consumer and isolated blackbox. Keep source,
installed, and live results separate.

Before review, prove the affected subset and every changed invariant:

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
- subscriptions are owner-scoped, bounded, exact where required, and one-shot;
- all pending eligible messages coalesce after 60 seconds into one attempt;
- failed wake and daemon restart never replay an attempted batch;
- one ID/subject/original-body delivery preserves a reusable direct-message
  lease and records one accepted native RPC; RPC acceptance proves only
  transport submission, while `collab recv` and its durable receipt prove
  consumption;
- successful resource/deadline delivery consumes exactly one
  matching one-shot subscription;
- release clears obsolete wait state; the daemon re-arms a registered identity's
  default direct-message lease from the current selected binding;
- no daemon-generated periodic continuation, inferred waiting, progress/ACK
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

Review only after the applicable tests and runtime evidence pass. A later
change invalidates only the conclusions it can affect: revalidate those
conclusions, not every prior review or runtime result. Integrate a reviewed
commit into latest main and run the affected main verification for that change;
install and live replay are separate results and are required only when the
change reaches the install or runtime boundary.

After a fix, verify the same user path again and classify the first divergence:

- `send` durable but no transport acceptance: inspect the selected transport,
  subscription, ownership, Agent state, and daemon log.
- tmux input submission appears but no worker result: inspect the durable
  mailbox receipt and peer state; do not call submission a reply.
- `recv` returns messages: the read is consumed atomically; no follow-up ACK is
  required. `msg`, `inbox`, and `context` remain read-only.
- task remains open: inspect owner identity, master responsibility, cleanup
  receipt, and notification supersession.

Record the tested source commit, installed version, daemon PID/socket, exact
commands, and live replay result in the bug system. A test pass without
latest-main merge, installed-binary verification, or applicable live replay
does not close the bug.
