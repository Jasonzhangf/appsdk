# AppSDK verified project truths

## Fix Lifecycle v2 — v0.1.3

- Required order: clean isolated Git worktree -> baseline reproduction -> committed fix candidate with positive/negative evidence -> architecture review PASS on exact commit/tree/diff/scope/map hashes -> unchanged-source post-review effectiveness replay -> exact-tree mainline merge -> promotion/compile/Active/Protected/Freeze.
- Playground is a logical mutable lifecycle. The physical execution boundary is a clean Git worktree. Absolute local worktree paths stay in `.appsdk-control/`; committed records contain portable IDs, refs, commits, hashes, scope, producer, and timestamps.
- `architecture_stable` requires only the architecture-phase graph. `verify` separately validates architecture-only, effectiveness-only, merge-ready, and full-promotion states. Freeze and publish always require the full graph.
- Exact merge means the candidate commit is reachable from the recorded merge commit, the merge is on the declared mainline ref, and the merged Git tree equals the reviewed/effectiveness-tested candidate tree. If mainline moved, rebase and repeat review/effectiveness.
- v0.1.3 is fail-closed by project SDK version. v0.1.2 consumers stay on `~/.local/lib/appsdk/0.1.2/appsdk` until an evidence-backed migration; do not synthesize PASS records.
- Global v0.1.3 binary: `~/.local/bin/appsdk` and `~/.local/lib/appsdk/0.1.3/appsdk`. Versioned bundle resources: `~/.local/share/appsdk/0.1.3/`. Global Codex skill: `~/.codex/skills/appsdk-project-governance/`.
- Final implementation evidence: Rust tests 21/21, release build PASS, global new-project/verify smoke PASS, DSH review `appsdk-fix-lifecycle-v2-r4` `VERDICT: PASS` with no P0/P1.
- Remaining design debt: module-registry owned-path coverage/import-edge enforcement is still a documentation skeleton and must not be described as complete machine truth. RouteCodex migration is a separate next phase.

## v0.1.3 delivery baseline — 2026-08-16

- Fix Lifecycle v2 canonical mainline commit is `7f62abe393f1e5ccc288d38b1f177ce72c5990b9`; the same commit is verified on `origin/main`.
- Installed global AppSDK is `0.1.3`; `~/.local/bin/appsdk` and `~/.local/lib/appsdk/0.1.3/appsdk` SHA-256 are `e3c36ae25c94d0c01c81cfe084fac7de8dc577f5ba3b8f91ae18b9d0587631a5`.
- Versioned bundle resources are installed at `~/.local/share/appsdk/0.1.3/`; Codex Skill is installed at `~/.codex/skills/appsdk-project-governance/`. Source/install directory diffs are empty.
- Mainline validation: Rust tests 21/21, release build PASS, global `new -> pin-lock -> verify` sample PASS, final DSH task `appsdk-fix-lifecycle-v2-final-20260816` returned `VERDICT: PASS` with no P0/P1.
- Delivery marker: `appsdk-v013-fix-lifecycle-mainline-delivered`.
- Public release: `v0.1.3` points to `7f62abe393f1e5ccc288d38b1f177ce72c5990b9`; downloaded `appsdk-0.1.3-macos-arm64` SHA-256 matches the installed binary.

## WRAI.TH cross-project coordination baseline — 2026-08-20

- A temporary relay is sufficient for capability testing, but formal multi-project testing requires a user-level WRAI.TH install: global `agent-relay` binary, launchd service, global MCP config at `~/.claude/.mcp.json`, `/relay` command, agent-relay skill, and activity hooks.
- AppSDK projects consume WRAI.TH through the global relay; AppSDK remains the governance truth for semantic claims, worktrees, evidence, review, merge, promotion, and publish.
- The installer must be run with project scanning disabled, then `agent-relay init --global` configures the global MCP connection. The runtime data directory `~/.agent-relay` must exist before launchd starts; otherwise the current WRAI.TH binary reports a misleading single-writer conflict because it cannot create the lock file.
- Verified global baseline: `agent-relay v1.11.0`, launchd `com.agent-relay` running on `127.0.0.1:8090`, `/api/health` returns `status: ok`.

## Review ordering correction — 2026-08-20

- Review is a post-verification functional and architecture audit, not a substitute for tests, build, install, restart, online smoke, or real-sample evidence.
- Required order for new candidates: keep the candidate tree uncommitted -> run candidate verification -> DSH Review; use Codex Review only when DSH is explicitly unavailable -> after explicit PASS create the candidate commit -> rerun unchanged-source effectiveness -> merge/queue.
- A review run before candidate verification, or against a tree changed after verification/review, is stale and cannot be used as evidence. This rule is encoded in the global AppSDK governance skill and the `0.1.4-beta.1` Codex/WRAI.TH beta skill/docs.

## v0.1.5 pre-review validation baseline — 2026-08-20

- Review admission now requires both development whitebox evidence and post-install/restart blackbox evidence through the deployed public entrypoint. Both evidence sets bind the exact candidate commit/tree, compiled artifact, producer identity, deployment environment, entrypoint, timestamps, and PASS result.
- The shared `assert_pre_review_validation_gate` is the unique owner for this identity/causality check and is called by both CLI review admission and architecture promotion. Source or artifact drift invalidates admission.
- Mainline commit `2a8059cc778efb32691bc749ffad95359d9ee87b` is pushed to `origin/main` and tagged `v0.1.5`.
- Verified release evidence: Rust tests 23/23, source-registry gate PASS, global binary SHA-256 `069c1b40071579432dacc30d5b731ea7e9a1c39c4062f07ced723b5c3858cbaf`, fresh deployed CLI blackbox PASS/fail-closed, and DSH review `dsh-1787240817469-801edec1` `VERDICT: PASS` with no P0/P1.
- Remaining P2: add direct positive/negative tests for `verify-sdk-source-registry`; later separate project-memory ownership semantics from SDK delivery if memory files become tracked lifecycle resources.

## Collab design decisions — 2026-08-26

- The first registered worker remains the intentional project-local `master`; this is a bootstrap policy, not an audit defect. Do not replace it with controller election or a lease unless Jason changes the policy.
- The framework owns sender identity. `Send` callers must not supply an arbitrary `from`; the daemon/session layer derives and injects the authenticated sender before recording the message.
- Collab is deliberately light-coupled: durable mailbox/journal is the truth, and communication should interrupt reasoning only for actionable requests, task assignment, required replies, blockers, or explicit state changes. Informational notifications stay available in the inbox and should not wake the agent by default.
- Codex-to-Codex communication must also pass through collab and its governance rules. Codex App Server/TUI protocol may serve only as the transport or wake-up adapter; native `spawn_agent`/`send_input`/`wait` semantics must not bypass collab task, message, identity, evidence, and quiet-delivery rules.
- Collab design hierarchy: (1) collaboration rules are above communication; (2) communication is a capability that must obey collaboration rules; (3) communication is intentionally restrained—evaluate actionability and interruption cost before sending/waking, so agent reasoning is not needlessly interrupted.
- Codex App Server activity should be used as collab presence input: probe/observe whether a bound Codex session is active, idle, blocked, not loaded, or unknown before deciding delivery. Offline handling may return a protocol-level queued/deferred status and prepare mailbox delivery for reconnect, but must never fabricate an agent reply or bypass collab rules.
- Target closed loop: Codex agents are managed through collab MCP; Codex runtime activity is observed for delivery decisions; heartbeat may probe/wake/resume a bound Codex session but must report `woken`, `resumed`, `unavailable`, or `queued` explicitly; non-Codex agents use tmux wakeups with mailbox durability; for Codex-to-Codex, mailbox is credential/evidence/history reference only, not the primary message transport.
- Registration design: register a common project-scoped agent identity first, then a runtime session, then one or more typed transport endpoints with declared read/write, push/pull, wake, presence, and resume capabilities. Codex `thread_id` is optional endpoint metadata; non-Codex agents use the same agent/session hierarchy with tmux and/or mailbox endpoints. Agents always call the unified collab interface; collab selects a declared channel according to collaboration rules and capability facts.
- Capability correction — Codex TUI does not expose the Codex Desktop thread tools such as `send_message_to_thread`, `read_thread`, or `wait_threads`. Never infer Codex communication capability from the runtime name. A TUI agent must report only tools/interfaces actually visible in its own context; absent a verified collab/App Server bridge, it must not register a Codex endpoint and should probe tmux, then mailbox. Desktop-only thread tools are not evidence for TUI capability.
- Codex source audit — upstream Codex has internal model tools `send_message`, `followup_task`, `send_input`, `resume_agent`, `wait_agent`, and `list_agents`, registered by `core/src/tools/spec_plan.rs` only when collaboration/multi-agent features are enabled. These are not named `send_message_to_thread`, and they target Codex agent paths managed by the internal `AgentControl`; they are model tool exposure, not automatically available to the TUI agent as external APIs. The TUI uses an embedded/local-daemon/remote App Server client internally; its client has `thread/list`, `thread/read`, `thread/resume`, and `turn/steer`, but a normal TUI does not expose those client methods as agent-callable tools. Collab integration must therefore detect the actual exposed tool list or an explicitly configured App Server bridge, not infer it from TUI source presence.
- Registration correction — communication capabilities are normalized and owned by collab; the target receives a projection of the normalized capability set. Any MCP server may provide outbound send, so sender transport is not the primary registration distinction. The critical target capability is arrival notification/delivery (push, wake, or only pull), which determines whether a message can reach the target without an agent actively polling. Do not update the collab skill with unimplemented capability schemas or interfaces; implement and test the protocol first, then update the skill from verified behavior.
