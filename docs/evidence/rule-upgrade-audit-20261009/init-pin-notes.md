# init-pin worker notes

Task: rule-upgrade-audit-20261009
Feature: aabed6a
Worker scope: C, ordinary init pin guard and Guidance proposal authorization semantics
Baseline: 3dfdaf8503b7a6f6a76651a1e282c038b6648c3a
Worktree: /Users/fanzhang/Documents/github/appsdk

2026-10-09 / start | Read live global AGENTS, coding-principals, codex-orchestrator worker contract, requirements, planner plan, task note, core report, and upgrade-observation report. | Sources listed above. | No product writes before this note.

2026-10-09 / observe | Confirmed core F1 and upgrade observation: ordinary init writes the current bundle before preserving an old project pin. | core/report.md; upgrade-observation/report.md. | Fix owner: init_project in rust/src/main/init.rs.

2026-10-09 / tests | Added public CLI regressions using the retained official 0.1.0011 archive. Added old-pin no-write rejection, official migration followed by init, repeated current-pin init idempotence, and proposal authorization assertions. | rust/tests/cli_smoke/part_07.rs; rust/tests/cli_smoke/part_13.rs. | No cargo run by contract.

2026-10-09 / implementation | Added a pre-write current-pin assertion in ordinary init and made the sdk lock writer fail typed on a mismatched pin. Synchronized bootstrap proposal readiness, questions, schema, instruction, post-authorization behavior, and next requirement to conditional authorization. | rust/src/main/init.rs; rust/src/guidance/intake.rs. | Error code: SDK_VERSION_MIGRATION_REQUIRED; next action points to pin-lock.

2026-10-09 / static check | rustfmt --check passed for all four changed files; git diff --check passed. | Commands recorded in report.md. | Cargo tests and controller red-green remain pending.

2026-10-09 / artifacts | Saved tests-only.diff, implementation.diff, and patch.diff under this run directory. | Hash values are in report.md. | Apply tests first for red evidence, then implementation for green evidence.

2026-10-09 / static check | Added explicit GIT_BUG_BIN removal to the manual init helper and regenerated patch hashes. | rust/tests/cli_smoke/part_07.rs. | No behavior change beyond test environment isolation.

Status: DONE for implementation and static self-check. Verification is delegated to controller by the task contract.
