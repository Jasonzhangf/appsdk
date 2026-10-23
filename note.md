# AppSDK work log

## 2026-08-26 Codex bridge feasibility

- External `@minhspark/codex-mcp-bridge` v1.11.1 was cloned into a temporary directory; its own suite passed 138 tests with 1 platform skip.
- A real `codex app-server --listen ws://127.0.0.1:8791` session was exercised through the bridge. The bridge created a thread, sent two sequential turns, preserved the codeword across turns, and read the same thread back. Smoke result: `PASS`.
- The tested integration boundary is the bridge-owned shared Codex App Server WebSocket. The bridge does not attach to the current Codex Desktop private stdio server or use `~/.codex/ipc/ipc.sock` as a message inbox.
- Next implementation boundary: add a Playground-only collab arrival adapter that invokes the external bridge/App Server endpoint; keep collab governance, cwd scope, message truth, and lifecycle ownership above the transport. Do not copy the third-party bridge source into AppSDK.
- Real TUI-to-TUI smoke passed: two tmux-launched Codex TUI clients connected to one shared WebSocket App Server; an external JSON-RPC `turn/start` addressed TUI-B's thread and the TUI-B pane rendered `Message from TUI-A transport test: reply exactly ARRIVED-B.` followed by `ARRIVED-B`. No tmux message injection was used. Both TUI threads were observed idle/connected before delivery.
- Test risk: the remote TUI displayed the App Server process cwd (`/Users/fanzhang/Documents/github/appsdk`) in its thread metadata even though the launcher cwd was a temporary directory. Cwd must be explicitly bound/validated before adopting this transport for collab isolation.

## 2026-08-14

- Current Rust CLI is the only governance implementation, but `pin-lock` binds only the binary digest. The release manifest has a separate contract digest and no installed docs/rules/skills contract.
- Before the current bundle work, `init`/`new` bootstrapped embedded contracts without installing the governance Skill or recording installed resource versions/digests.
- Completed target: the globally installed SDK is a versioned bundle contract (binary + contracts + docs + rules + skills); project initialization installs the Skill/resources and generated resource metadata is verifiable. Manual project files remain AI-owned; generated metadata is SDK-owned and fail-fast validated.
- RouteCodex is not modified in this phase. Its old local binary currently accepts an artifact that the current AppSDK binary rejects, confirming compiler-version drift to solve through the global bundle migration later.
- The release binary is installed at `/Users/fanzhang/.local/bin/appsdk`; the versioned global Bundle resources are installed at `/Users/fanzhang/.local/share/appsdk/0.1.0/`.

## 2026-08-15

- v0.1.2 released (commit `7c0e3b2`, tag `v0.1.2`, asset `appsdk-0.1.2-macos-arm64`):
  previous-active artifact validation no longer compares the immutable
  previous Active artifact's `build` command against the current module
  contract; module identity, `artifact_paths`, and the artifact's own signed
  hash are still verified, and current module artifacts keep the strict build
  comparison. Red test extended in `begin_version_preserves_v1_and_opens_a_version_bound_source_stage`
  (change build args after v2 verify, then compile-module and freeze) - green.
- Version identity bumped to `0.1.2` (Cargo, embedded bundle manifest,
  scaffolds, `version` subcommand, docs). `cli_smoke` 20/20, fmt clean.
- Installed globally at `/Users/fanzhang/.local/bin/appsdk`
  (sha256 3685149eab60ed887737e1ff0c9a6ddbbd0add32424d40595e27296ebf7b8686);
  bundle resources mirrored to `/Users/fanzhang/.local/share/appsdk/0.1.2/`.
- RouteCodex V4 edge re-freeze/publish using this release: verified in a
  scratch worktree first, then the real `v4` lifecycle (pin-lock, freeze
  active-v2, publish-active active-v2, verify, verify --admission).

## 2026-08-15 P2 fix (DSH review pass follow-up)

- DSH review `appsdk-v0.1.2-release-dsh-r1` PASS (VERDICT: PASS, exit 0),
  P2 only: `templates/minimal/.appsdk/sdk.lock` missing the two bundle digest
  placeholder keys, so template-derived draft projects fail
  `INVALID_SDK_BUNDLE_DIGEST` (pre-existing; CI does not exercise templates).
- Fixed in `56518a9`: template lock now emits
  `bundle_digest`/`bundle_manifest_digest` placeholders matching
  `write_project_scaffold`. Red→green reproduced: old lock fails draft verify,
  fixed lock passes.

## 2026-08-15 Fix Lifecycle v2

- Jason approved the required order: clean Git worktree reproduction/fix -> architecture audit -> post-audit effectiveness replay -> mainline merge.
- Current v0.1.2 gap: host worktree contract is unbound; record graph resolves one evidence file per module; architecture review is generic; no post-review effectiveness record; no Git reachability/merge identity gate.
- Chosen design: add Worktree/Reproduction/FixCandidate/Effectiveness/Merge records, extend architecture Review and Promotion references, resolve every evidence ID, and require exact commit/tree/scope ordering before `architecture_stable`.
- Implemented as AppSDK v0.1.3. `architecture_stable` now proves clean isolated worktree, baseline reproduction, committed candidate, positive/negative candidate evidence, exact architecture review inputs/map hashes, and AI confidence. Effectiveness and merge remain explicit later phases; `verify` accepts architecture-only and effectiveness-only legitimate intermediate states.
- Full promotion/freeze/publish graph re-runs architecture, effectiveness, and merge gates. Effectiveness uses post-review positive/negative/blackbox evidence; exact merge requires candidate ancestry and merged Git tree equality with the reviewed candidate tree.
- Compatibility is fail-closed: project SDK version must match the running 0.1.3 binary. Versioned global binaries retained at `~/.local/lib/appsdk/0.1.2/appsdk` and `~/.local/lib/appsdk/0.1.3/appsdk`; latest global entry is `~/.local/bin/appsdk`.
- Verification: JSON parse and diff checks pass; Rust tests 21/21; release build passes; global 0.1.3 new-project/verify smoke passes at `/tmp/appsdk-0.1.3-r4.AOckxE/project`; installed binary SHA-256 before final no-code review was `e3c36ae25c94d0c01c81cfe084fac7de8dc577f5ba3b8f91ae18b9d0587631a5`.
- Final DSH review `appsdk-fix-lifecycle-v2-r4`: `VERDICT: PASS`, no P0/P1. Remaining P2: release binaries are untracked user artifacts; canonical/template record schemas are duplicated semantically; module-registry coverage is still a documentation skeleton; merge negative tests do not yet cover every ancestry/tree variant.

## 2026-08-16 Fix Lifecycle v2 delivery

- Existing v0.1.3 candidate was moved from the dirty main tree into one declared clean Playground worktree, verified, architecture-audited, effectiveness-replayed without source changes, then fast-forwarded to main.
- Main commit `7f62abe393f1e5ccc288d38b1f177ce72c5990b9` pushed to `origin/main`; remote identity verified.
- Global binary and versioned bundle reinstalled. Binary SHA-256: `e3c36ae25c94d0c01c81cfe084fac7de8dc577f5ba3b8f91ae18b9d0587631a5`. Global `new -> pin-lock -> verify` live sample passed.
- DSH review `appsdk-fix-lifecycle-v2-final-20260816`: `VERDICT: PASS`, no P0/P1. Worktree and temporary branches removed only after remote verification; claim released.
- GitHub release `v0.1.3` published from commit `7f62abe`; downloaded `appsdk-0.1.3-macos-arm64` hash exactly matches the installed/reviewed binary.

## 2026-10-08 Collab Master authority delivery

- User requested a GCM implementation worker, main merge, rebuild and restart. PR #16 merged as `bb9ce30b0efe60ab48445fcf4b3efd9c13c40fc4` after independent AGY PASS and six green CI jobs.
- Master authority now has Empty/Assigned states. Typed grants are the sole authority owner. Approved clear/replacement does not probe incumbent liveness. Status/context/board/panel share the grant projection. tmux transport uncertainty does not revoke authority.
- Main official installation produced 0.2.0257. Official down/up succeeded. New canonical daemon PID 46300 answered status. Installed public consumers passed 32 tests; real isolated native AppServer registration passed. Author suites passed 925 Collab and 18 MCP tests, with one ignored Collab test.
- Current Desktop control socket still refuses connections. Direct Desktop identity recovery is not verified; the error remains in the receipt. Installation did not clear or replace formal Master grants. Live message-count observations changed concurrently and are not claimed unchanged.
- Delivery index: `docs/evidence/collab-master-authority-fix-20261007/README.md`; main receipt: `main-runtime.md`; frozen contract and DAG: `docs/design/collab-master-authority-contract-20261007.md` and `docs/dagpipe/collab-master-authority.graph.json`.
- Nine owned worktrees/branches and 21 isolated worker homes were reclaimed without force. Other task worktrees and dirty primary documents were preserved. The clean main clone's final removal and defect closure are recorded in the canonical external task note after the receipt push. Necessary raw result logs remain as task evidence.

## 2026-10-09 SDK rule upgrade and project development guidance

- AppSDK 0.1.0014 was delivered at `9dc90303278ddd2a8be5fab8e835d498057ebe6e`.
  Rule/Skill audit evidence: `docs/evidence/rule-upgrade-audit-20261009/`;
  final installed, review, CI, release and cleanup receipts:
  `/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/closeout-delivery.md`.
- The release record reports affected local verification and independent review;
  the full tag gate passed 531 AppSDK and 28 DAGPipe tests. Cleanup remains
  PARTIAL: eight test roots were recreated by Collab timers. Product delivery
  does not establish cleanup completion or authority for a shared daemon reset.
- Jason requested root project rules and a development Skill. Added root
  `AGENTS.md` for SDK architecture/features/ownership and
  `.agents/skills/appsdk-dev/SKILL.md` for project commands, affected verification,
  rule-upgrade audit and official product delivery; README links both entrypoints.
- These documents govern source development. They do not initialize a managed
  consumer contract. The CI selector still widens unspecified Rust paths to the
  whole AppSDK package; this limitation is recorded rather than claimed fixed.
- This is a documentation/Skill change under the current fast path. Validation
  scope is Skill structure, references and Git diff; no product tests, release
  build, installer or daemon operation is required by these changes.
- `skill-creator/scripts/quick_validate.py` passed. All local Markdown links in
  the new entrypoints and README resolve; the tracked diff check passed.

## 2026-10-10 Release documentation, platforms and channels

- Jason specified three release concerns: root README introduction/routes/
  governance start/three-module operation, Windows/Linux/macOS, and npm channels.
- Read-only observation confirmed v0.1.0014 has macOS ARM64 AppSDK and Memory
  assets plus Skills/SHA256SUMS. Linux has existing Ubuntu release-gate evidence;
  this release has no Linux/Windows binary assets. No npm package exists.
- Windows source gaps include AppSDK Unix-only/no-op lock branches and reset
  helper compilation boundary, shell installers, and Collab Unix IPC/host
  transport/process management. These are static findings, not new build or
  runtime test results; no platform support is declared from this observation.
- Reworked README into the public product/start/documentation entrypoint and
  added `docs/design/release-distribution.md` with facts, target matrix, owner
  boundaries, fixed artifact/version flow, npm packaging proposal and increments.
  Root AGENTS and appsdk-dev route platform/channel changes to that document.
- This increment delivers documentation and a release implementation proposal.
  Platform ports, matrix workflows, npm packages and registry publishing remain
  unimplemented; Windows host transport capability remains open.
- Jason selected the npm scope/name `@jsonstudio/appsdk`; registry versions and
  publishing account access still need current evidence.
- Public registry metadata query returned E404 on 2026-10-10. This is not proof
  of scope ownership or publishing permission. Skill structure and all local
  Markdown reference targets passed checks; no product tests were run.

## 2026-10-11 — Collab same-pane Context completion owner

- Valid existing master/peer restoration in a nonresident project could commit
  Register but return `registered=true / outcome=partial`. Context completion
  read project binding/default lease from the host owner instead of the selected
  project runtime. It now uses the registered app scope/project owner and the
  host current-thread route. Existing identity, grant and generation fences stay
  with their owners; same-pane restoration requires no additional promotion.
- Removed the wrongly placed host binding/lease checks and kept existing failure
  semantics. The actual outer journal append error is no longer discarded. No
  new recovery policy, approval workflow, fault hook or native creator was added.
- Old installed Collab 0.2.0261 reproduces both opaque partial completion and the
  discarded error (`Null`) through the same public CLI fixtures; the candidate
  passes those counterexamples. Validation: 19 focused source cases, 6 tmux CLI,
  2 recovery wire, 1 authority security wire and 4 failure/replay CLI cases pass.
  One overall independent review and its sole finding recheck completed PASS.
- `PEER_LIFECYCLE_CREATE_NATIVE_REQUIRED` is the existing AppServer-only native
  creation capability boundary for a tmux actor, not host approval rejection.
  Original living panes were verified; old daemon `unknown` is not proof of lost
  registration and is not claimed independently fixed by this owner correction.
- Source and raw evidence:
  `/Volumes/Intel/playground/appsdk/.worker-runs/collab-pane-recovery-20261011/`.
  Installation/shared daemon/live/remote evidence is recorded there separately;
  source tests and review alone do not prove those delivery endpoints.

## 2026-10-12 — Collab durable identity, explicit roles and selected-path registration

- Candidate evidence follows requirements ledger v8 / communication v4.
  Tmux identity uses the persisted socket+pane anchor. PID/session drift,
  busy/idle, capture/probe failures and endpoint reachability no longer revoke
  identity or role. One explicit master/peer intent overrides the current
  role mapping through the existing authority owner.
- Ordinary and committed staged restoration reuse the original identity,
  binding/adaptor and receipt. They perform no new Register, initial admission,
  loopback or binding generation. Manual communication recovery has its own
  loopback; failure retains identity, role, grant and binding.
- Initial registration requires a genuine participant return on the path the
  daemon selected. Async continuation retains exact intent and owner fences
  while allowing unrelated ledger activity. Late AppServer/DSH replies finish
  the original operation. Original manager publication and compensation retain
  terminal failures across duplicate replies and restart. New command IDs are
  scope-qualified; saved legacy IDs stay unchanged.
- Completed normal context repeats return the complete current project state
  without repeating committed effects. Query remains pure. Endpoint and agent
  activity are independent of durable identity through status, context, board,
  dashboard and close/orphan consumers. A reachable tmux pane keeps endpoint
  true while unknown agent activity remains unknown. Only exact missing-target
  probe results permit the existing Missing close exemption; current authority,
  responsibility, task, unread and actor/target fencing stay enforced.
- Pre-integration native receipts: peer244/244, host-route119/119,
  remaining binary586 passed with one preexisting live ignore, timer47/47,
  genuine registration12/12, late native return10/10, AppServer public16/16,
  Context other54/54 plus cancellation2/2, board/HTTP/physical endpoint4/4.
  Old failure logs remain, and socket-denied child execution never counts as
  acceptance. The final declared Collab CI and finding-specific review are
  recorded separately from remote integration, installation and runtime.
- Evidence and DAG audit:
  `/Volumes/Intel/playground/appsdk/.worker-runs/tmux-identity-20261011/`.
  Main and foreign candidates retain their unrelated requirements/governance
  work. No reset, identity impersonation, real-peer message or memory write is
  part of this repair. Exact remote, installed and cleanup receipts are kept
  in the evidence root after their respective actions.

## 2026-10-19 — Collab board queue, claim and display names

- User scope: master/peer publication, self claim, optional child claim,
  queued pending work, completion/blocker continuation and idle recheck.
  Children normally receive peer-parent/master assignments without an extra
  publish/claim sequence. Existing parent/master send authority is retained.
- Added atomic `board claim`, shared execution-slot/resume checks and typed
  MCP adapter. Blocked/waiting retain resource and close responsibilities.
  Managed public work stays visible under parent responsibility; private
  tasks, child names and identity facts stay out of public board projections.
- Added self `worker name <name>|--reset`. Defaults derive from the typed
  current role and bound pane: master Jenny, ordinary tmux peer pane ID.
  Explicit labels persist through reload/compaction and role changes; reset
  and retirement remove them. Task/message/grant IDs and routing are unchanged.
- Author verification: 998 Collab core tests passed, one pre-existing ignore;
  25 MCP, 22 AppServer/board, 5 peer lifecycle and 4 subworker CLI tests passed.
  Original and additional privacy red results were preserved. Formatting,
  JavaScript syntax and Skill validation passed. Updated graphs remain SESE.
- Installed canonical Collab/Collab-MCP and Skill: 0.2.0275. Isolated real tmux
  and browser checked pane/Jenny defaults, explicit labels, filter/search,
  safe text rendering, MCP naming, reset and formal daemon reload persistence.
  Shared daemon was formally reloaded from an existing declared operator
  route after this unregistered source-root route was refused; no identity
  was initialized. Review and remote integration are pending.
- Owner evidence: `/Volumes/Intel/playground/appsdk-board-run-20261019/`.
  `verification.md` distinguishes static Skill walkthroughs from live command
  behavior; this change adds no periodic worker wake or automatic execution.
  Main's pre-existing dirty changes and foreign worktrees remain protected.
- Follow-up: corrected default Jenny in narrowed lifecycle Read using typed
  scoped master authority. Corrected managed Send's inner authorization to
  accept the original parent or current master. Direct dispatch reuses the
  shared execution-capacity predicate; blocked tasks retain their resources.
  Both regressions have failing red evidence. All 246 peer protocol tests
  passed, and the public Create/Bind/MCP Send/Recv test passed with ordinary
  parent dispatch and blocked-next continuation. Refreshed official installed
  binaries/Skill to 0.2.0276 and formally reloaded the shared and isolated
  daemons. Installed worker Read shows Jenny. Independent whole-milestone
  review and follow-up closure review passed with no P0/P1 findings. Local
  guidance audit identified historical digest/candidate-binding instructions;
  removed those retired requirements. Remote integration is pending.
- CI caught two integration omissions: the tmux authority-clear fixture still
  expected master-only publication, and the board graph referenced unregistered
  operator version 2. Updated the fixture to require peer publish success and
  master-only invite rejection; restored the registered version 1 bindings.
  All 7 tmux authority CLI cases and the embedded graph registry consumer test
  passed. These corrections change tests/design bindings only; installed
  runtime code is unchanged.

## 2026-10-20 — r6 review finding: worker status binding diagnosis

- Independent review found one P1 in `collab/src/server/mod_parts/part_10.rs`:
  `identity-mismatch` worker status returned only
  `selected transport does not prove the registered identity; verify its binding`,
  so the typed failure surface did not identify the binding contract.
- Fix: worker status now returns
  `SESSION_THREAD_BINDING_MISMATCH: selected transport does not prove the
  registered identity; preserve the registered binding and use `collab context`
  as the only repair entry; do not edit or infer the binding`.
- Tests in `required_fields_tests.rs` now assert immutable worker-status
  diagnosis: `identity_valid=false`, `status=identity-mismatch`, typed
  `SESSION_THREAD_BINDING_MISMATCH`, binding-preservation wording, `collab
  context` as the repair entry, and no mutation of tasks, project bindings,
  workers, subscriptions, runtime journal, or host journal. Existing
  post-repair identity and board assertions remain.
- Red evidence: `red/worker-status-binding-diagnosis-red.log`.
- Focused green: `green/worker-status-binding-diagnosis-green.log`.
- A full-suite attempt without the CI harness ran binaries in parallel and
  omitted test binaries; its 11 failures were environmental/isolation failures,
  not product findings: `red/public-binaries-without-harness-parallel-red.log`.
- Correct full binary suite (serial, with COLLAB_TEST_BINARY and
  COLLAB_TEST_MCP_BINARY): `collab` 1048 passed, 1 ignored;
  `collab-mcp` 27 passed: `green/post-review-finding-full-binary-suite-green.log`.
- Commit: `2bfc8277 fix(collab): preserve binding in worker status diagnosis`.

## 2026-10-20 — stable names and runtime closure

Names own project work; the agent sessionID is its current execution carrier.
Current name/session recovery, fixed-pool allocation, confirmed Jenny takeover,
Jenny adjudication, open-task/resource retarget, unread named-mail inheritance,
finite peer patrol recovery and Codex presence/activity now share the declared
owner boundaries. Master and peer orchestrate at different goal levels;
one-shot execution/observation goes to GCM subworkers. Tmux only locates probes
and delivery and never supplies a name or identity key.

Late public consumers exposed and closed role/name delegation and clear,
self-unregister peer recovery, grant-effect receipts, route-less tmux close,
and denied-reset pre-authorization mutation. Current source evidence:
1117 main unit tests, 26 MCP tests, 47 primary consumers, 64 context contracts
and all 14 additional consumer targets pass via affected-scope reuse/retests;
one existing unit test remains ignored. Graph, source-format and raw red/green
receipts are in `~/.collab/runs/name-runtime-closure-20261020/verification.md`.
This entry is source/candidate evidence. Independent milestone review,
integration and formal installation follow. The global daemon is user-managed;
no agent up/down/restart or tmux/pane lifecycle action was performed.

Combined latest main 39d836aa and retained upstream recovery/status and
read-only delivery behavior. Combined coverage passed 1320 Collab cases and
16 graph gates, with one existing ignored case. Snapshot fixture readiness
now waits for its independent background adjudication notices before the
unchanged full-journal purity assertions.

Independent whole review found four P1 gaps. Repairs retain project/name
occupancy revision history across release/replay/snapshot, restore current
daemon route selection before exact read-only credentials, and remove the
remaining tmux identity and peer-all-busy GCM restrictions from source/global
Skills. New public red cases reproduced repeated unread-mail handoff loss and
same-session historical-credential ambiguity; affected green results are in
`finding-mailbox-result.md` and `finding-reader-result.md` under the run.
Independent findings-only follow-up closed all four P1s with no new findings;
host schema classification passed. Final shared units: 1124 main passed,
27 MCP passed, one existing ignored. Integration and installation are recorded
separately in the run, including final-install.log and remote delivery receipt.

Linux CI exposed C13's independent unknown-presence notification racing its
post-restart journal baseline. The test now completes the exact seed name
occupancy/binding notification before making credentials stale. Cancellation,
query/retry and complete journal equality assertions remain intact. Exact C13
and the full 64-case context target pass locally; only the test file changed.
Installed/runtime product version remains 0.2.0300. User restarted the daemon;
new PID57606 responds and the existing RouteCodex project's seven worker
identities are valid. Linux CI confirmation and remote merge remain pending.

User added peer patrol reassessment: peers own patrol tasks/subscriptions for
their ongoing goals and evaluate/update existing patrol scope on every master
assignment. The shared role brief carries this at registration, recovery and
board invitation; source/global Skill and stable-name reference agree.
Registration/recovery public red then 6-case green passed. Formal 0.2.0301
installed and 8 installed registration/endpoint cases passed. Loaded daemon
remains 0.2.0300 until the final user maintenance activation.

Second Linux CI passed C13 and all 64 context cases. Its next first failure
was the endpoint projection test asserting periodic agent facts before the
first observation. Added an exact project/worker/session fact readiness wait;
all endpoint and agent-state assertions remain. Both endpoint cases pass.

Third Linux CI reached Snapshot and exposed a production receipt bug: the
first notification attempt accepted a batch, but a later empty-batch skip
appended a failure for another message already accepted in that batch. The
notification owner now preserves current-carrier acceptance on a skipped
attempt. A pending name-handoff wake still records its own failure. A new
deterministic red reproduced the erroneous journal write; all 15 batch units
and all 8 public peer lifecycle cases pass after repair. The return class and
actual delivery/consumption behavior remain explicit. Further review, final
build activation and Linux confirmation follow in the run.

Receipt repair passed independent targeted review and final shared units
(1126 main, 27 MCP, one existing ignored); official 0.2.0302 installed.
Fourth CI passed Snapshot and reached a second periodic-presence consumer.
The two observation consumers now share one exact current-session readiness
helper. Candidate endpoint2, peer8 and master-authority8 pass. The rebuild
consumer now identifies its exact message among legitimate existing patrol
mail and still checks its true consumption receipt. Release consumers exclude
only the test-build fault-injection case; that case remains required/passing
on the hook-enabled candidate. Final installed evidence is retained separately.

Main feature candidate a6c7b603 passed all applicable remote CI. User then
required active patrol shutdown: master/peer must close owned patrol tasks and
timed subscriptions after completion or when no new task/actionable work
remains, without recreating idle loops. Direct-message reception remains.
One shared runtime rule and source/global Collab/recovery/migration Skills now
state this. Public registration red reproduced the missing rule; registration6
and authority8 pass after the text change. Official 0.2.0303 is installed;
final user activation and this small addition's CI remain pending.

## 2026-10-21 — owner-scoped goals and active prompt ablation

User correction: master and peer are orchestrators at different goal levels.
Each manages its own goal; master proxy registration is invalid. The previous
master-only goal documentation described the defect instead of fixing it.
Runtime now scopes goal records/locks by canonical project and stable name,
shares one current context for identity/role, permits ordinary peer finite goal
deadlines, and keeps peer deadlines separate from master wake. Legacy singleton
data is only imported for the authenticated owner; unusable optional legacy
data is preserved/reported without blocking a new independent goal.

Active source prompt owners, generated references, SDK Skills/templates and
global goal-prompt were audited. Twelve conflict/duplication classes and their
owners are recorded in docs/design/appsdk-prompt-audit-20261021.md. The formal
Collab installer also retires two exact stale reference copies found in the
installed global Skill; arbitrary user references are preserved. Current
stable-name design text now matches the goal/master-idle boundary.

Evidence: ~/.collab/runs/owner-scoped-goals-20261021/verification.md and raw logs.
Latest AppSDK units59; public owner/prompt consumers39; Collab goal units12;
registration6 and authority8; installer6. Canonical installed cross-product
AppSDK -> private daemon -> native fixture passes1 in122.66s: peer owns goal,
deadline consumed, peer cancel preserves master and direct-message reception.
Official binaries and managed Skills installed; Collab0.2.0305. Global daemon
activation remains user-managed. No production registration, message, timer,
daemon lifecycle or tmux lifecycle action was performed. Independent overall
review and main integration follow; shared dirty main/other worktrees protected.

Independent milestone review found three P1 closure gaps: remote pending goal
subscriptions did not follow name handoff; legacy singleton reads omitted the
original invocation worktree; and native consumers asserted polling consumption
without exact native submission. All three now have red/green evidence in
review-fix-* logs. Candidate public consumers5 pass in123.97s, including real
AppSDK same-name A->B status/rearm/cancel, old caller denial, schedule/history
preservation, linked-worktree legacy upgrade and exact native wake/consumption.
Native matcher recognizes start toolOutput and steer/queue input formats;
rejecting-adapter negative case remains. A parent red also caught transport-lost
goal subscriptions omitted from handoff; valid restored schedules now transfer
and rearm without reviving terminal states. Name units60 and final AppSDK units59
pass. Official Collab0.2.0307 installed; final installed consumers/finding recheck
and integration continue in the same run. Global daemon remains user-managed.

## 2026-10-21 Context-owned session recovery migration

Context now evaluates explicit approved targets before archive resolution,
converges duplicate same-session delivery carriers during registration and
restoration, and keeps an ordinary peer's existing name and work. Historical
pane placement cannot select an archive. Cold archives without a current worker
re-register from current facts; foreign-project targets remain isolated.
Migration uses the existing name owner-start/cancellation boundary. Shipped
Collab and AppSDK migration Skills remove agent-owned archive selection,
cleanup and manual migration prerequisites.

Evidence: ~/.collab/runs/session-recovery-replacement-20261021/ contains the
plan, SESE graph validation, red/green cases and independent review/recheck.
Final public context71, name70 and resolver28 pass; prior shared-state1139
pass/one existing ignored. Independent recheck PASS closes all three original
P1 findings. Installation, main integration and user-managed daemon activation
remain separate delivery facts; no global daemon or tmux lifecycle was touched.

## 2026-10-23 DSH public context registration

CLI context and MCP collab_context now accept a typed DSH capability report
and pass it to the existing daemon registration owner. Session-only restoration
reuses a unique saved scoped identity. Explicit gateway API/target conflicts
cannot recover an old credential through the draft path. Missing first-use
facts are reported before operation admission. Initial registration adds no
message challenge; gateway admission still verifies actual reported facts.

The public consumer starts without an identity, checks the committed DSH
binding and default lease, repeats and restarts without a new registration,
rejects API conflict without replacing the saved credential, then verifies
send/receive/linked reply/read acknowledgements. Gateway queue acceptance
remains distinct from receiver consumption. MCP and invalid-report consumers
use the same real CLI/daemon path. All four DSH public cases pass, alongside
identity30, context29, MCP27, DSH/adaptor52, Desktop2 and serial context-public73.
A real current gateway admission plus session-only restore also passes through
an isolated daemon; no target messages or global daemon changes were made.

Evidence: ~/.collab/runs/dsh-registration-delivery-20261023/. The inherited
worktree and tests were preserved, then combined with current main. Shared
units initially passed1154 with six failures: one missing candidate binary
parameter and five fixture failures from an obsolete no-identity error-code
assertion. Scoped rechecks pass12 plus the exact staged-restore consumer1.
The read-only test still rejects identity selection from a fake TMUX string.
Final review, remote CI, main integration and formal installation are recorded
separately; installation does not activate the user-managed global daemon.
