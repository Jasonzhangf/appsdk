# Main delivery receipt

Functional main: `bb9ce30b0efe60ab48445fcf4b3efd9c13c40fc4`.
Reviewed candidate: `5ee3755ee23a4e206987d9253190964d53f045e3`.
Their repository content diff is empty. The later receipt commit changes only
documentation and evidence. It does not change the installed source inputs.

PR [#16](https://github.com/Jasonzhangf/appsdk/pull/16) merged at
2026-10-08 10:13:58 UTC. The matched-head admin merge exited 0.
`merge-receipt.json` is the actual GitHub receipt. Both candidate CI runs passed
format, test and release, as recorded in `final-ci-checks.json`.

## Build and formal lifecycle

The clean main checkout used the official `scripts/install-global-collab.sh`.
Session 54941 exited 0. `main-install.log.gz` contains the full output.
The installed version is `collab 0.2.0257`. The installer compared the installed
binaries with its build outputs and checked all seven repository Skill files.

- CLI SHA256: `4ce24cd6f9e9c2955f8a98f23814c886393cb9ee6047df589e395fb6c53f170e`.
- MCP SHA256: `6531d062111e7ee80b24b74d13b1b89a01735381374261c81cc2e4a82698e349`.

The official lifecycle is canonical `collab down` then `collab up`, from the
formal daemon's RouteCodex cwd. Down exited 0. Old PID 14298 disappeared and the
formal socket was released. Up exited 0. New PID 46300 was verified with Darwin
libproc: executable `/Users/fanzhang/.cargo/bin/collab`, cwd RouteCodex.
Canonical `collab status --all` exited 0. No non-target daemon was restarted.

## Installed entry acceptance

Session 6570 exited 0. Both test binary variables selected the canonical
installed CLI and MCP. The actual nonzero test counts were AppServer 15, DSH 1,
master status 2, MCP 1, tmux authority 7 and tmux receipt/context 6: total 32.
`main-installed-public.log.gz` preserves the complete output.

The final DSH case includes an unreachable gateway. The MCP case uses a fresh
process and real initialize, initialized notification and tools/call exchange.
Scope isolation, replacement, clear, empty replay, same-principal rebind,
old-generation refusal, dashboard GET agreement and business-state preservation
are exercised through public entry points. Author binary suites separately
passed 925 Collab tests plus 18 MCP tests; one Collab test was ignored.

Session 88369 ran the existing real AppServer first-registration script with
canonical COLLAB_BIN and exited 0. `main-installed-native.log` reports PASS.
It started no model turn and sent no production message. The parent verified
the owned `/tmp/cg.uSdT42` fixture was absent after script cleanup.

## Preservation and limits

`main-preservation.json` compares public before/after snapshots. AppSDK has zero
tasks and one peer. RouteCodex has 113 tasks and 48 peers. Durable task fields,
peer IDs with active task assignments and Master control fields are unchanged.
Task keepalive observation is excluded explicitly. No formal grant was cleared
or replaced by installation.

RouteCodex message-count observations changed from 4 before installation to 1
after main restart. There was concurrent live activity. This receipt makes no
claim that live message contents or counts were unchanged. Isolated public
consumers separately prove clear does not delete their tasks and messages.

Direct current Desktop `collab context` exited 1 because
`~/.codex/app-server-control/app-server-control.sock` refused connections.
`formal-main-context.stderr` preserves APPSERVER_ENDPOINT_REJECTED /
ADAPTER_ROUTE_UNAVAILABLE and os error 61. Current Desktop identity recovery is
not PASS. Isolated native context capability is PASS. Existing Desktop MCP
processes were not claimed to hot reload; a fresh installed MCP process passed.

## Resource receipts

Eight worker worktrees and the original candidate were removed normally.
All nine task branch refs were deleted with normal `git branch -d` after merge.
No forced reset, checkout, stash, branch deletion or worktree removal was used.
`worker-worktree-cleanup2.log.gz` records the eight worker receipts.
`failed-fixture-cleanup2.log.gz` records 13 owned failed fixtures with scoped
shutdown and absence checks. A stale socket file was distinguished from a live
listener by ECONNREFUSED before removing the owned fixture root.
`home-cleanup.json` records removal of 21 isolated task homes. Credential/config
symlink targets were retained. No task worker process remained.

The clean main clone remains the receipt-writing owner until this documentation
commit is pushed. Its cleanup endpoint is clean Git state, exact remote receipt,
then directory removal and absence check. The canonical external `note.md` gets
that final result. Necessary notes, raw result logs and review receipts remain
as evidence under the task-owned `.worker-runs` directory. Other worktrees and
the new formal daemon are retained. The primary tree's original dirty
`docs/collab.md` is preserved.

`final-run-notes.md`, `retrospective.md` and `retrospective-check.md` archive
owner facts and the completed qualitative retrospective. They do not create a
second code review PASS or change product requirements or managed memory.
