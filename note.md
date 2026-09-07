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
