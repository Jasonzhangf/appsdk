# AppSDK active prompt audit and ablation

## Contract and scope

The user explicitly requires master and peer to own their own long-horizon
goals. They have the same orchestration duties at different goal levels. A
peer is not a bounded execution worker. Master must not proxy-register a peer
goal. This correction also requires an audit of all active AppSDK project
prompts for conflicts, incomplete logic and competing rule owners.

The effective correction is recorded in `req-collab-globalized-orchestration-patrol`
version 2. Older ledger changes and immutable migration/release snapshots remain
historical evidence, not current operating instructions.

This audit covers the active source paths below and their installed projections.
It distinguishes generated copies from independently maintained rule sources.
Source changes do not by themselves prove installation or loaded-daemon behavior.

## Prompt owners and projections

| Surface | Authoritative source | Projection or consumer | Disposition |
|---|---|---|---|
| Project architecture and identity facts | `AGENTS.md` | development agents | Name owns work, sessionID is carrier; self-owned master/peer goals |
| Current stable-name design | `docs/design/collab-stable-name-work-ownership-20261020.md` | implementation and audit readers | Own-goal boundary matches runtime; only master-idle remains master-only |
| Runtime goal prompt, role charter, fleet and notification guidance | `rust/src/long_horizon_policy.rs`, `long_horizon_role.rs` | `appsdk goal prompt/subscribe`, `longhorizon show` | Both roles orchestrate; direct bounded GCM dispatch; no master proxy or unregistered-execution blocker |
| Goal command feedback and status | `rust/src/main/goal.rs`, `longhorizon.rs` | public AppSDK CLI | Resolve current owner before reading/writing; no foreign status mutation; neutral `goal_prompt` field |
| Registration/recovery role instructions | `collab/src/server/mod_parts/part_06/master_recovery.rs` | current daemon role brief | Shared self-goal rule for master and peer; explicit receipt and idle patrol closure |
| Generated project Collab reference | `collab/src/scope.rs::COLLAB_DOC` | newly created project reference | Remove frozen task/command workflow copy; route to current Skill and role brief |
| Idle wake and mailbox action guidance | `collab/src/server/keepalive.rs`, `mailbox.rs` | daemon notifications | Preserve role-independent bounded dispatch, owner-scoped patrol, receipts and user-managed lifecycle |
| Collab operating procedures | `collab/skills/collab/` | embedded bundle → formal installer → global Skill | Remove master-only/Desktop goal prohibition; preserve actual project arbitration boundaries |
| SDK governance entry and command catalog | `sdk-skill-sources/appsdk-project-governance/SKILL.md`, `references/command-surface.md` | AppSDK installer → global Skill | Dispatch by task type; each orchestrator owns its goal |
| Setup and state guidance | governance `references/init-prompts.md`, `state-paths.md`, `bootstrap-migration.md` | setup/migration prompts | Remove liveness-as-authority, offline reset sequence, lifecycle command exposure and proxy registration |
| SDK goal prompt reference | governance `references/goal-prompt.md` | requested goal prompt generation | Keep AppSDK additions; reusable generic template belongs to global `goal-prompt` Skill |
| Bounded executor and process harness | governance `references/subagents-config.md`, `process-control-harness.md` | execution dispatch | Remove all-peers-busy prerequisite; preserve independent role/model and worktree boundaries |
| Guidance intake and projection | `rust/src/guidance/intake.rs`, `projector.rs`, governance `appsdk-guidance.json` | selected Guidance workflow | Reuse decisions/authorization; graph/gate/red ordering; no routine freeze or difference ledger; lifecycle owner remains explicit |
| Consumer project rules | `templates/minimal/AGENTS.md` | initialization/resource refresh | Project facts only; remove candidate packaging and duplicate difference-ledger requirement |
| Migration instructions | `sdk-skill-sources/appsdk-migration/SKILL.md` | formal installed migration Skill | Reference current name contract; user-confirmed Jenny takeover wins; own goal never transfers to master |
| Memory Skill | `sdk-skill-sources/project-memory/` | formal installed memory Skill | Auxiliary retrieval/promotion owner; does not decide identity, goal authority or runtime scheduling |
| Host orchestration and goal template | global `codex-orchestrator`, `goal-prompt` | host task dispatch | Existing independent GCM/Plan/Review routing remains; goal template now explicitly rejects proxy registration |

The installed global Skills are derived products. They must be updated by the
owning official installer, rather than maintained as separate implementations.
Host discovery links do not constitute extra rule owners.

## Confirmed contradictions removed

1. AppSDK accepted only a live tmux master for goal registration. Daemon
   admission and timers independently rejected or suppressed peer goals.
   Both layers now accept the ordinary peer's own finite goal.
2. A singleton local goal let a peer's status reconciliation modify the
   master's state. Record and advisory-lock scope now use canonical project
   and stable owner name. Worktree/session changes preserve that scope.
3. Peer charters and goal prompt names projected an execution worker or
   master-exclusive role. Public role output is peer; generated goal prompts
   state the shared orchestration function.
4. Runtime policy, SDK Skill references and generated project documentation
   required all peers to be busy before master could use GCM. Bounded work
   now routes directly to independent GCM by task type.
5. Desktop was prohibited from registering goals and peer registration was
   redirected to master. Host placement no longer grants or removes goal
   responsibility; neither role may mutate another owner's goal.
6. Setup references treated an unreachable master as unassigned, prohibited
   explicit takeover of a live holder, and mixed project reset with global
   daemon shutdown. Authority, reachability and lifecycle now have separate
   owners; only global lifecycle remains user-managed.
7. Agent-facing SDK references exposed daemon maintenance commands and obsolete
   reset flags. They now route to the current reset/migration owner without
   exposing user-only lifecycle commands or credentials.
8. Guidance/templates requested repeated confirmation, an exhaustive difference
   ledger and ordinary freeze/source-identity ceremony. They now reuse valid
   decisions and evidence and retain only risk-appropriate checks. The older
   goal-clarification design is narrowed to its explicitly enabled record
   workflow and reuses existing authorization; it no longer imposes another
   universal development gate or a project-wide stop for local ambiguity.
9. The generated Collab project reference and governance goal reference copied
   complete command/workflow templates. Those copies are reduced to facts and
   owner references, so upgrades have fewer independent rule bodies to drift.
10. Briefing re-read context and separately queried master status, so an
    unreachable secondary observation could turn a current master into unknown.
    Role and goal selection now share one authoritative context. Peer assigned
    tasks/blockers are filtered to that caller; project observations do not
    become assignments to the peer.
11. The installed Collab Skill retained retired `commands-board-and-master.md`
    and `recovery-and-reset.md` after upgrade. They duplicated the current
    Skill and contained obsolete dispatch/recovery instructions. The existing
    installer cleanup allowlist now includes those exact retired files;
    unrelated user references are preserved and non-force installs do not
    delete files. The real installed directory comparison exposed this gap.
12. Two unreferenced legacy plans still prescribed a fixed goal template with
    repeated confirmation/freeze and a persistent managed-child launch path.
    `docs/goals/appsdk-goal-prompt-generation-plan.md` and
    `collab/docs/subagent-config-plan.md` are removed. Neither had incoming
    references. The current host goal template, SDK goal reference and Collab
    Skill remain the unique rule owners; no replacement procedure is added.

Unowned or malformed legacy singleton data is preserved and reported, but is
never imported and cannot veto a separate authenticated owner's explicit goal.
Current owner-scoped record corruption remains an explicit error. A legitimate
legacy worker-ID owner matching the authenticated caller maps in memory to its
stable name; the original legacy bytes are not rewritten.

Legitimate master-only project arbitration, project reset, master-idle and
cross-project operations remain. Legacy managed-subagent compatibility records
still grant no new production execution authority. Internal legacy wire names
do not turn an ordinary peer into a bounded execution worker.

## Evidence and remaining boundaries

Independent review found three closure gaps in the first candidate: pending
goal subscriptions did not follow a name to its new execution carrier; legacy
singleton reads omitted the actual invocation worktree; and the native consumer
asserted mailbox consumption without first asserting exact native submission.
These are runtime and evidence defects, not additional role restrictions. The
goal/subscription diagrams now require those edges. Repair and focused review
evidence are retained under the same run with the `review-fix-` prefix. The
initial candidate's PASS claims cover mailbox behavior only and must not be
read as proof of native wake or complete name-carrier continuation.

Evidence is retained in the task run
`~/.collab/runs/owner-scoped-goals-20261021/`: independent plan, original
observations, public red failures, goal/legacy/managed regression logs,
12 goal-deadline unit cases, native peer deadline consumer and the explicit
cross-product AppSDK → daemon → native-thread consumer. Final combined
verification, installation and independent review append their actual results
there; this document does not claim a future PASS.

Other active/dirty worktrees are protected task resources. Their older source
copies are not silently rewritten or treated as installed truth. Existing
consumer-specific custom documents and published historical snapshots remain
unchanged; upgrades use the current source/Skill owners. No global daemon or
tmux lifecycle action is part of this audit.
