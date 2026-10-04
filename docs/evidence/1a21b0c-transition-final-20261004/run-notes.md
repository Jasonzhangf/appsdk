# 1a21b0c transition final composition author run notes

Owner: unique GCM implementation author (final composition)
Worktree: /Volumes/Intel/playground/appsdk/1a21b0c-transition-final-20261004
Branch: codex/1a21b0c-transition-final-20261004
Base / HEAD: 9a9d674a52faf1b90b7e4b62fe8d01fac3d3572a (fix(sdk): authorize witnessed historical pin targets (a7cc2a4))
Toolchain: cargo/rustc 1.97.1; global `dagpipe` 0.1.1
Allowed writes this task: rust/tests/cli_smoke/main.rs (compose part_20 before part_21) and docs/evidence/1a21b0c-transition-final-20261004/**

## Node 1 - composition equivalence (DONE)

Time: 2026-10-04T05:52Z
Conclusion: Applying the frozen successor patch to base 9a9d674 produces a tree whose
only difference from this worktree is rust/tests/cli_smoke/main.rs (the one approved
`include!("part_20.rs");` line) plus the leftover `main.rs.rej` in the scratch copy.
No other product/test/evidence byte differs, so all old product changes remain
byte-identical to the base+patch composition.
Evidence: scratch tree /tmp/1a21b0c-verify.RzLMjZ built from `git archive 9a9d674`
plus `successor-frozen.patch`; `diff -rq` reported only main.rs (and .rej). main.rs
diff = single added include line.
Input/version: base 9a9d674; patch
/Users/fanzhang/.codex/task-evidence/agentteams/receipts/1a21b0c-transition-impl-20261003/successor-frozen.patch
Next: run the required gates.

## Node 2 - cargo fmt --check (DONE)

Time: 2026-10-04T05:55Z
Conclusion: exit 0, no output.
Evidence: logs/cargo-fmt-check.log
Input/version: worktree HEAD 9a9d674 + composed delta

## Node 3 - cargo build --bin appsdk (DONE)

Time: 2026-10-04T05:55Z
Conclusion: exit 0. One pre-existing, unrelated warning: unused import
`super::validation::*` in src/communication/store_runtime.rs (untouched by this
change). Built executable: appsdk 0.1.0010 (rust).
Evidence: logs/cargo-build-appsdk.log
Input/version: CARGO_TARGET_DIR=/tmp/1a21b0c-final-20261004/target

## Node 4 - cli_smoke pin_lock_ --list (DONE)

Time: 2026-10-04T05:56Z
Conclusion: exit 0; "34 tests, 0 benchmarks" for the `pin_lock_` filter. Derived
combined name set (not the old standalone 27). Includes all 7 new transition cases
(part_20.rs) and the part_21.rs a7cc2a4 pin cases.
Evidence: logs/cli-smoke-pin-lock-list.log
Input/version: same composed candidate

## Node 5 - cli_smoke pin_lock_ run (DONE)

Time: 2026-10-04T05:56Z
Conclusion: exit 0; "34 passed; 0 failed; 0 ignored; 0 measured; 301 filtered out".
All 7 transition tests pass:
  pin_lock_refreshes_trusted_legacy_zone_transition_contracts
  pin_lock_rejects_unknown_zone_transition_content
  pin_lock_rejects_schema_shaped_zone_transition_alias
  pin_lock_preserves_non_declared_schema_alias
  pin_lock_refresh_is_idempotent_and_preserves_migration_history
  pin_lock_cleans_failed_transition_staging_and_resumes_partial_pair
  pin_lock_creates_missing_supported_zone_transition_paths
4 mapped writer regressions also pass inside this run (explicit reuse by name):
  pin_lock_reconciles_matching_authoring_bundle_mirror
  pin_lock_rejects_drifted_authoring_bundle_mirror_before_migration
  pin_lock_restores_missing_record_contract_directory
  pin_lock_migrates_stale_project_record_contracts
Evidence: logs/cli-smoke-pin-lock-run.log

## Node 6 - mapped writer regression (5th, standalone) (DONE)

Time: 2026-10-04T05:57Z
Conclusion: exit 0; "1 passed; 0 failed". Exact name:
pinned_sdk_witness_is_executable_and_resolvable_in_a_fresh_worktree
Evidence: logs/cli-smoke-pinned-sdk-witness.log

## Node 7 - verify-sdk-source-registry from / (DONE)

Time: 2026-10-04T05:57Z
Conclusion: exit 0; `{"ok":true,"gate":"sdk_source_registry"}`. The prior base
dependency (SDK_SOURCE_LINE_LIMIT collab/src/identity.rs:1826>1500) is resolved on
this base: collab/src/identity.rs is 1388 lines and unchanged by this candidate.
Evidence: logs/verify-sdk-source-registry-from-root.log
Input/version: candidate appsdk retained at retained/appsdk, run from cwd /

## Node 8 - appsdk dagpipe validate (DONE)

Time: 2026-10-04T05:57Z
Conclusion: exit 0; compiled_registry true, single_source_single_sink true. Graph id
set == manifest id set, exactly 10 ids (diff of sorted id lists empty).
Evidence: logs/appsdk-dagpipe-validate.json

## Node 9 - existing pin-history graph validate (DONE)

Time: 2026-10-04T05:58Z
Conclusion: exit 0; "valid DAG: appsdk-sdk-pin-history@0.1.0 (6 nodes, 5 edges, 6 waves)".
Evidence: logs/sdk-pin-history-graph-validate.log
Input/version: global dagpipe 0.1.1

## Node 10 - fingerprint freeze and evidence-reuse decision (DONE)

Time: 2026-10-04T05:57Z
Conclusion: Product/fixture fingerprints frozen (logs/frozen-hashes-and-delta.log).
Comparison against the prior implementation receipt source_sha256:
  producer.rs       96a9de39...  EQUAL
  reset_governance.rs 89f1f36f... EQUAL
  part_09.rs        45b08425...  EQUAL (restored to base)
  part_20.rs        3a1e7312...  EQUAL
  fixtures x3       a6468f12 / 6c485a13 / 45686684  EQUAL
  migration.rs      d5682cd9(old) vs 3b29d6d8(now)  NOT EQUAL
  main.rs           12181a18(old) vs 13858b37(now)  NOT EQUAL (approved include)
Because migration.rs is not equal (base 9a9d674 advanced migration.rs via a7cc2a4),
the product fingerprints are NOT fully equal; the previous red-before-fix and
full-archive results are therefore NOT reused or relabeled as current execution.
No red/full-archive run is part of this task's validation list.
Evidence: logs/frozen-hashes-and-delta.log

## Node 11 - retained executable (DONE)

Time: 2026-10-04T05:57Z
Conclusion: candidate appsdk 0.1.0010 retained externally for primary integration.
Path: retained/appsdk
sha256: c17d46e38f29b7153d534fc221132989be4e00a64d2ce5acc35a1c173b7bdbd1
Evidence: logs/retained-appsdk-hash.log

## Outstanding / non-claims

- Canonical installation is NOT performed and is outstanding (forbidden this task).
- No commit, merge, push, install, daemon restart, OTA, memory write, Collab, or
  architecture review was performed.
- git diff --check clean (logs/git-diff-check.log).

## Node 12 - resource cleanup (DONE)

Time: 2026-10-04T05:58Z
Conclusion: no active cargo build/rustc compile; own temp target
/tmp/1a21b0c-final-20261004 (removed) and scratch composition tree
/tmp/1a21b0c-verify.RzLMjZ (removed); both `test ! -e` pass. Retained executable
preserved at retained/appsdk.
Evidence: logs/resource-cleanup.log
