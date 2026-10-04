# 1a21b0c transition-refresh implementation run notes

Status: `IMPLEMENTED`, `PUBLIC_GATES_GREEN`, `CANDIDATE_AWAITING_SPECIFIC_DEPENDENCY`.

Repair status: `SCOPE_TWO_FIXED`, `PUBLIC_FOCUSED_GATES_GREEN`,
`RELEASE_REGISTRY_BLOCKED_BY_BASE_PATH_LIMIT`,
`HISTORICAL_ARCHIVE_DEPENDENCY_RETAINED`.

This is an implementation-only record. It does not claim commit, merge, push,
architecture review, installed-runtime delivery, Teams admission, or full
SDK/AgentTeams closure.

## Scope and identity

| Item | Value |
| --- | --- |
| Worktree | `/Volumes/Intel/playground/appsdk/1a21b0c-transition-impl-20261003` |
| Branch | `codex/1a21b0c-transition-impl-20261003` |
| Base / `origin/main` | `61e8c534b5295fb0f60a6634ad3cf99d99f12a4c` |
| Admitted design | `docs/evidence/1a21b0c-transition-diagnosis-20261003/report.md` |
| Admitted design tree | `3391baa9633124da5e982f93fe00114152fafad1` |
| Candidate binary (retained copy) | `.tmp/retained/appsdk-1a21b0c-9e3b57da` |
| Candidate binary SHA-256 | `9e3b57daf60286396a938151ec2277c870c561bd16e9b5b39d575dfd61ec6cb5` |
| Candidate version | `appsdk 0.1.0010 (rust)` |

Tooling read from live environment: `rustc 1.97.1 (8bab26f4f 2026-07-14)`,
`cargo 1.97.1 (c980f4866 2026-06-30)`, `dagpipe 0.1.1`,
`git 2.50.1 (Apple Git-155)`, macOS `26.6.2 (25G83)`.

## Source identity

Modified (working-tree SHA-256 / git blob):

| Path | SHA-256 | Git blob |
| --- | --- | --- |
| `rust/src/main/migration.rs` | `d5682cd93521773f520d405956600684ab7b609f691e72edc27efaa0222f0add` | `3e53e31368057605d7c1bcd78a9c9118e0db3208` |
| `rust/src/main/producer.rs` | `96a9de39e7d9312ceca1a71cc38b807cc0e41cc0bc7b43a16926f778727f7f6e` | `70edb2fb056fda747ee952a921a7800807ddfb02` |
| `rust/src/main/reset_governance.rs` | `89f1f36fa9b55134f66b431abaf3d1d1683f40bc496dcc70339f67f605b8485b` | `84f987b8d22c4d7b18b2c9f484b0929226410b3f` |
| `rust/tests/cli_smoke/part_09.rs` | `a946b6dc988be90423e703f39285222233d9f29d68a95d05498010bc5b31de24` | `1a72e1c5fcc24e6210b319130473463d0e7d0344` |

Added fixtures (exact official predecessor bytes):

| Path | SHA-256 | Source evidence |
| --- | --- | --- |
| `rust/tests/fixtures/zone-transition-0.1.3.json` | `a6468f12b64d3e0125ddd77828a4eeeee48cf3a38a0ee6d5bfe56935cd8a1957` | versioned tag `v0.1.3` |
| `rust/tests/fixtures/zone-transition-0.1.4.json` | `6c485a138ab5a657b760969be42b167ebd034f8a43446609505c2f5d16d5afab` | versioned tag `v0.1.4` |
| `rust/tests/fixtures/zone-transition-0.1.5-0.1.6.json` | `4568668437b4e0b44db4709d27e31c2783c8a6e4ccd828273a4675775f69ca1f` | versioned tags `v0.1.5` / `v0.1.6`; reproduced AgentTeams `de099b79` |

## Node log

### 2026-10-03 / design-admission read

- Status: complete.
- Input: admitted report plus owner/call-edge source in `migration.rs`,
  `reset_governance.rs`, `producer.rs`, `governance.rs`, `init.rs`, tests, and
  `docs/dagpipe/sdk-pin-history.graph.json`.
- Conclusion: missing publication belongs to `pin_lock` current-step
  materialization; validator is not the owner. Single shared refresh plan plus
  existing `atomic_write_bytes` owner is the admitted mechanism.
- Next: implement preflight, materialization, staging cleanup, and black-box
  gates inside the allowed write paths only.

### 2026-10-03 / implementation

- Status: complete.
- Input: base `61e8c534`; admitted design.
- Changes:
  - `migration.rs`: `preflight_current_transition_contracts` plus
    `install_current_transition_contracts`; closed three-digest official
    predecessor allowlist; canonical declared path or legacy alias declared
    path; canonical sibling written first, declared path last; non-declared
    sibling is not in the refresh set and is not created.
  - `reset_governance.rs`: preflight immediately after project/version
    validation and before early migration project publication; refresh during
    current-step materialization before final lock/project publication.
  - `producer.rs`: the single existing `atomic_write_bytes` removes only its own
    staging file on write/rename failure, preserves the original error, accepts
    missing staging, and surfaces cleanup failure plus path when cleanup fails.
  - `part_09.rs`: six admitted public gates plus a missing-path coverage test.
- Next: prove red-before-fix and green-after-fix on the public CLI.

### 2026-10-03 / red-before-fix

- Status: complete.
- Input: isolated `.tmp/red-base-20261003b` built from `git archive HEAD` (base
  production source confirmed byte-identical to `HEAD` for `migration.rs`,
  `producer.rs`, and `reset_governance.rs`) with only the new `part_09.rs`
  tests and exact predecessor fixtures copied in.
- Evidence: [red-before-fix.log](logs/red-before-fix.log) - all six design
  gates fail with exit 101 against pre-fix production code:
  stale canonical bytes after trusted refresh, unknown content accepted,
  declared schema alias accepted, non-declared schema alias stale canonical
  behavior, idempotence failing at `verify`, and write-failure cleanup not
  failing as designed.
- Conclusion: each admitted gate is red before the fix and exercises a real
  behavior difference, not a source-structure assertion.

### 2026-10-03 / green public tests

- Status: complete.
- Input: candidate source above, `CARGO_TARGET_DIR=.tmp/cargo-target`,
  `APPSDK_HOME=.tmp/appsdk-home`, `TMPDIR=.tmp/tmp`.
- Evidence: [new-public-tests.log](logs/new-public-tests.log) - all seven
  tests `1 passed; 0 failed`.
  - `pin_lock_refreshes_trusted_legacy_zone_transition_contracts`
  - `pin_lock_rejects_unknown_zone_transition_content`
  - `pin_lock_rejects_schema_shaped_zone_transition_alias`
  - `pin_lock_preserves_non_declared_schema_alias`
  - `pin_lock_refresh_is_idempotent_and_preserves_migration_history`
  - `pin_lock_cleans_failed_transition_staging_and_resumes_partial_pair`
  - `pin_lock_creates_missing_supported_zone_transition_paths`
- Write-failure gate uses a real owned immutable target (`chflags uchg`), real
  `rename` failure, `nouchg` restoration in the guard, and no mocked
  `fs::rename`, test-only production switch, or timed-kill success claim.
- Next: affected shared-writer regressions.

### 2026-10-03 / shared-writer regressions

- Status: complete.
- Evidence: [existing-writer-regressions.log](logs/existing-writer-regressions.log)
  - all five `1 passed; 0 failed`:
  - `pinned_sdk_witness_is_executable_and_resolvable_in_a_fresh_worktree`
  - `pin_lock_reconciles_matching_authoring_bundle_mirror`
  - `pin_lock_rejects_drifted_authoring_bundle_mirror_before_migration`
  - `pin_lock_restores_missing_record_contract_directory`
  - `pin_lock_migrates_stale_project_record_contracts`
- Next: format, build, and graph checks.

### 2026-10-03 / static checks and graph governance

- Status: complete.
- Evidence:
  - `cargo fmt --check`: exit 0, empty output
    ([cargo-fmt-check.log](logs/cargo-fmt-check.log)).
  - `cargo build --bin appsdk`: exit 0, one pre-existing unrelated warning
    `unused import: super::validation::*` in `communication/store_runtime.rs`
    ([cargo-build.log](logs/cargo-build.log)).
  - `appsdk dagpipe validate` from source cwd, parent cwd, and `/`: exit 0;
    all 9 current manifest IDs exactly once; `single_source_single_sink: true`,
    `compiled_registry: true`
    ([source](logs/dagpipe-validate-source-cwd.log),
    [parent](logs/dagpipe-validate-outside-source-cwd.log),
    [root](logs/dagpipe-validate-root-cwd.log)).
  - `dagpipe graph validate docs/dagpipe/sdk-pin-history.graph.json`: exit 0;
    `valid DAG: appsdk-sdk-pin-history@0.1.0 (6 nodes, 5 edges, 6 waves)`
    ([log](logs/sdk-pin-history-graph-validate.log)).
- Next: single full-archive dependency probe.

### 2026-10-03 / full-archive dependency probe

- Status: stopped at known upstream dependency; no bypass.
- Input: frozen read-only archive
  `/Volumes/Intel/playground/appsdk/a7cc2a4-pin-impl-20261003/docs/evidence/a7cc2a4-pin-impl-20261003/fixture-agentteams-de099b79.tar.gz`,
  SHA-256 `2d91c66612c307e2ba76ceba188ca613e557840d69b9ef1568bb33825e68db50`;
  extracted unchanged into `.tmp/dependency-probe-b/extract`; isolated
  `APPSDK_HOME`/`TMPDIR`.
- Observed pre-pin project: SDK `0.1.6`, declared legacy alias, both root
  transition hashes `456866...ca1f`.
- Public commands:
  - `appsdk pin-lock <fixture> --binary <candidate>` -> exit 1,
    `INVALID_SDK_MIGRATION_RECORD`
    ([log](logs/full-archive-dependency-pin-lock.log)).
  - `appsdk verify <fixture>` -> exit 1, `NON_CANONICAL_RECORD_CONTRACT_SET`
    ([log](logs/full-archive-dependency-verify.log)).
- Side effects: `.appsdk/project.json` unchanged
  `5e87b3e29c43792078d09e6f0757ff3b64dc33f4778c52437e70c38e6b979e43`;
  `.appsdk/sdk.lock` unchanged
  `930fb338517760fded3e7414d16e9e433f04624518f05f4dd9c01dc47af42c52`;
  both root transition files still `456866...ca1f`; no transition staging
  residue.
- Conclusion: this standalone base stops before the root-refresh owner at the
  distinct frozen historical-pin candidate dependency. The dependency is
  recorded once and was not weakened, bypassed, copied in, or relabeled.
- Terminal: `CANDIDATE_AWAITING_SPECIFIC_DEPENDENCY` for the full unchanged
  archive pin -> verify acceptance. Public current-consumer fixtures carrying
  genuine old root bytes validate this root-refresh contract.

### 2026-10-03 / evidence and staging

- Status: complete.
- Evidence: [checksums.txt](receipts/checksums.txt),
  [implementation-receipt.json](receipts/implementation-receipt.json).
- Explicit allowed-path staging followed by `git diff --cached --check`; see
  receipt for the exact staged path list and result.
- No commit, merge, push, review, install, daemon restart, memory write, or
  other worktree touched.

### 2026-10-03 10:03 PT / review-repair-input frozen

- Status: complete.
- Input/result: HEAD remains `61e8c534b5295fb0f60a6634ad3cf99d99f12a4c`;
  all production files are staged only. Before repair, `part_09.rs` was 2059
  lines (`a946b6dc988be90423e703f39285222233d9f29d68a95d05498010bc5b31de24`)
  and its exact candidate addition began at line 1493. The existing candidate
  binary remained `.tmp/retained/appsdk-1a21b0c-9e3b57da`
  (`9e3b57daf60286396a938151ec2277c870c561bd16e9b5b39d575dfd61ec6cb5`).
- Conclusion: confirmed both scoped source-review defects: unconditional
  `chflags` execution and `MAX_SOURCE_LINES=1500` overflow. The third
  historical-archive finding remains a separate dependency and is not copied
  into this candidate.
- Next: restore base `part_09.rs`, move only the candidate block to
  `part_20.rs`, gate only macOS immutability, then rerun affected public gates.

### 2026-10-03 10:11 PT / test relocation and platform gate

- Status: complete.
- Input/result: explicit patch removed only the candidate block from
  `part_09.rs`; byte comparison proved `part_09.rs` equals base
  `149245574e25d487f706577464393e3c4f773a951f77a3608a5d557006300354` and
  `part_20.rs` equals the prior candidate addition. `main.rs` includes
  `part_20.rs`.
- Portable/macos-only selection: six ordinary pin-lock public tests and all
  helpers remain unconditional and Linux-enabled. `ImmutableFileGuard`, the
  `chflags`-based write-failure fixture, and only
  `pin_lock_cleans_failed_transition_staging_and_resumes_partial_pair` are
  annotated `#[cfg(target_os = "macos")]`. No Linux execution is claimed from
  macOS host compilation.
- Next: focused public tests, formatting/build, source registry, DAG validation,
  and diff checks.

### 2026-10-03 10:36 PT / scope repair verification

- Status: two scoped defects fixed; focused public behavior verified.
- Final layout:
  `rust/tests/cli_smoke/part_09.rs` is exactly the base
  `149245574e25d487f706577464393e3c4f773a951f77a3608a5d557006300354`
  (`45b08425e4bda707fc0487cebcf0f117fe58576b8af535287018d138d133afeb`,
  1,492 lines). `rust/tests/cli_smoke/part_20.rs`
  (`3a1e73123b2f94406bb3a24fe4a2dbf022e6f5f5672c9c55e76f4cfd64bdfb45`,
  571 lines) contains the prior candidate addition. `main.rs` includes it.
- Platform gate: only `ImmutableFileGuard`, `assert_no_zone_transition_staging`,
  and `pin_lock_cleans_failed_transition_staging_and_resumes_partial_pair`
  are `#[cfg(target_os = "macos")]`. The other six new public tests and shared
  helpers remain unconditional. This is recorded as selection, not Linux
  execution; no Linux runtime was available here.
- Linux compile check: `cargo check --test cli_smoke --target
  x86_64-unknown-linux-gnu` passed with `zig cc` as the cross compiler
  ([log](logs/linux-cross-check-final2-after-review-repair.log)). This proves
  the portable test set type-checks under Linux cfg, but is not a claim of
  Linux execution.
- Evidence: [focused tests](logs/focused-public-tests-final2-after-review-repair.log)
  ran `pin_lock_` with 27 passed / 0 failed, including all seven new tests and
  the four `pin_lock_` writer regressions. The fifth mapped writer regression
  `pinned_sdk_witness_is_executable_and_resolvable_in_a_fresh_worktree` passed
  separately ([log](logs/pinned-writer-regression-after-review-repair.log)).
- Format/build: `cargo fmt -- --check` exit 0; build exit 0 with only the
  pre-existing unused import warning
  ([fmt](logs/cargo-fmt-check-final2-after-review-repair.log),
  [build](logs/cargo-build-final-after-split.log)).
- DAG/diff: `dagpipe graph validate docs/dagpipe/sdk-pin-history.graph.json`
  exit 0; both `git diff --check` and `git diff --cached --check` exit 0
  ([DAG](logs/sdk-pin-history-graph-validate-after-review-repair.log),
  [diff](logs/git-diff-check-final-after-review-repair.log)).
- Registry: the current production candidate reaches
  `SDK_SOURCE_LINE_LIMIT:collab/src/identity.rs:1826>1500`
  ([log](logs/verify-sdk-source-registry-retained-binary.log)). The file and
  registry manifest are unchanged from base; `collab/src/identity.rs` is
  1,826 lines at `HEAD`
  ([base line count](logs/base-61e8c534-line-limit.log)). This base-owned
  release blocker is outside the allowed write scope, so the source-registry
  acceptance remains `INCOMPLETE`; the moved `part_09/part_20` files are now
  below the 1,500-line rule.
- Product hashes: the migrated production source hashes are unchanged:
  `migration.rs d5682cd9...`, `producer.rs 96a9de39...`,
  `reset_governance.rs 89f1f36f...`.
- Retained executable: `/tmp/appsdk-1a21b0c-final-executable/appsdk`,
  SHA-256 `55b1428be7eda20f3aa5e58725e53e3020133b79123399b3a58c4e538ff05902`,
  version `appsdk 0.1.0010 (rust)`. Task-owned Cargo cache and temp directories
  were removed; no `uchg` flags remain. The previous `.tmp/retained` copy was
  superseded and removed.

## Cleanup and retained obligations

- Removed own transient `.tmp` directories and superseded retained binary.
- Retained one tested executable outside the repository at
  `/tmp/appsdk-1a21b0c-final-executable/appsdk`
  (`55b1428be7eda20f3aa5e58725e53e3020133b79123399b3a58c4e538ff05902`).
- Retained source fixtures under `rust/tests/fixtures/` as committed test input.
- Retained the frozen `a7cc2a4` tree untouched; no worktree removal there.

## Boundaries and remaining dependency

- Current base `61e8c534` fails the unchanged `verify-sdk-source-registry`
  release gate on `collab/src/identity.rs:1826>1500`; this repair cannot touch
  that path or the registry manifest.
- This tree does not contain the distinct frozen historical-pin candidate and
  does not claim full unchanged-archive closure.
- Final unchanged full-archive `pin-lock -> verify` acceptance remains required
  after the separate pin source composes with latest `main`.
- No full SDK/Teams admission, architecture review, commit, merge, push, OTA,
  install, or daemon work is claimed here.

## Primary r2 evidence consumption / 2026-10-04T02:52:18.949388+00:00

Author exceeded the bounded execution time while rechecking unchanged receipts after required validations. Primary stopped only exact owned author PID75966; wrapper75913 and child75919 are also ESRCH. This is not an author terminal-completion claim. All nine source/test/fixture SHA256 values and the retained binary SHA match the author receipt; public pin filter27/27 and mapped writer regressions5/5 are consumed actual results. Both diff checks pass. Production bytes are unchanged from the separately validated full-archive composition; composed-archive-acceptance.tar.gz preserves that entire report/raw evidence and composed-archive-primary-receipt.json binds its source hashes. The temporary mixed composition was never a delivery commit and has been normally removed after byte-equal export.

Strict source admission still fails unchanged base identity.rs1826>1500, tracked by f7bf558. Separate baseline repair, exact architecture review, integration/push and canonical install remain pending. Current candidate binary is retained outside Git at /Users/fanzhang/.codex/task-evidence/agentteams/receipts/1a21b0c-transition-impl-20261003/r2-primary-consumption/retained/appsdk, SHA25655b1428be7eda20f3aa5e58725e53e3020133b79123399b3a58c4e538ff05902; author receipt original /tmp path is historical provenance and will be reclaimed. Reuse valid source/public evidence until actual affected inputs change. No current SDK closure or Teams installed MVP claim.
