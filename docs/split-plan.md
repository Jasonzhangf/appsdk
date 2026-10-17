# Retired physical source-line split plan

Status: retired. Physical source-line limits no longer apply. Source registry
verification retains unique active ownership and forbidden-path checks.
The inventory and split order below are historical planning evidence, not
current work or a required split loop. Refactoring must follow actual module
responsibilities and behavior needs.

Owner: appsdk master (codex-%3)
Worktree: playground/split-large-files-1500
Branch: fix/split-large-files-1500
Historical limit: 1500 lines per tracked .rs file (retired).

## Historical oversized .rs inventory (>1500)

| lines | file |
|---|---:|
| 25569 | rust/tests/cli_smoke.rs |
| 23728 | collab/src/server/mod.rs |
| 23054 | rust/src/main.rs |
| 12702 | collab/src/server/peer_tests.rs |
| 8665 | rust/src/communication.rs |
| 7638 | rust/tests/communication_cli.rs |
| 4974 | collab/src/main.rs |
| 4703 | collab/src/server/global_state.rs |
| 4056 | collab/src/migration.rs |
| 3962 | collab/src/server/state.rs |
| 3397 | collab/src/adapters/codex_app_server.rs |
| 2943 | collab/src/identity.rs |
| 2674 | collab/src/server/timers.rs |
| 2601 | rust/src/global_registry.rs |
| 2536 | collab/src/scope.rs |
| 2402 | rust/src/memory.rs |
| 1911 | rust/src/dagpipe.rs |
| 1733 | collab/src/subagent.rs |
| 1524 | collab/src/reset.rs |
| 1506 | collab/src/server/mailbox.rs |

## Historical split order (by owner, smallest first)

1. collab/src/server/mailbox.rs -> mailbox/ (mailbox module dir)
2. collab/src/reset.rs -> reset/ (reset module dir)
3. collab/src/subagent.rs -> subagent/ (subagent module dir)
4. rust/src/dagpipe.rs -> dagpipe/ (dagpipe module dir)
5. rust/src/memory.rs -> memory/ (memory module dir)
6. rust/src/global_registry.rs -> global_registry/ (global_registry module dir)
7. collab/src/server/timers.rs -> timers/ (timers module dir)
8. collab/src/identity.rs -> identity/ (identity module dir)
9. collab/src/adapters/codex_app_server.rs -> adapters/codex_app_server/ (adapter module dir)
10. collab/src/server/state.rs -> state/ (state module dir)
11. collab/src/migration.rs -> migration/ (migration module dir)
12. collab/src/server/global_state.rs -> global_state/ (global_state module dir)
13. collab/src/main.rs -> split CLI dispatch and appserver init
14. rust/src/communication.rs -> communication/ (communication module dir)
15. rust/tests/communication_cli.rs -> split tests by area
16. collab/src/server/peer_tests.rs -> split tests by area
17. collab/src/server/mod.rs -> extract server sections into new modules
18. rust/src/main.rs -> extract governance/lifecycle/verification into modules
19. rust/tests/cli_smoke.rs -> split by CLI subcommand area

## Historical owner map (module-registry)

- rust/src/main.rs: runtime-core
- rust/src/communication.rs: communication
- rust/src/dagpipe.rs: dagpipe-dag-contracts / appsdk::dagpipe
- rust/src/memory.rs: project-memory
- rust/src/global_registry.rs: runtime-core
- collab/** : collab-runtime
- rust/tests/** : verification-tests

Current source ownership remains declared in
`contracts/maps/module-registry.json`. Use the project development Skill for
checks that apply to a concrete behavior change.
