# worker notes - requirements-session-lock (source registry test split)

- worker: independent GCM worker (parent = current goal orchestrator); no Collab registration
- worktree: /Volumes/Intel/playground/appsdk/requirements-session-lock
- base: 8555a74e5e46de10136fda9539ae044d3e8bdfbc
- goal: remove SDK_SOURCE_LINE_LIMIT:collab/src/server/global_state_tests.rs:1597>1500 by a mechanical test split only. No product behavior change, no threshold change, no test deletion, no blank-line trimming.
- write scope: collab/src/server/global_state_tests.rs + one same-owner part file
- non-goals: docs/collab.md, Rust CLI, other workers' files; no commit/push/install/restart/worktree

## node 1 - recon (2026-10-07)

commands + evidence:
- `appsdk verify-sdk-source-registry .` -> `SDK_SOURCE_LINE_LIMIT:collab/src/server/global_state_tests.rs:1597>1500`
- `wc -l collab/src/server/global_state_tests.rs` -> 1597
- owner: contracts/maps/module-registry.json module_id=collab-runtime, owner=collab::runtime, owned_paths=["collab/**"], no source_line_limit -> default 1500
- existing include pattern: global_state_tests.rs:36 `include!("global_state_tests_part2.rs");` (part2 = 474 lines); state_tests.rs:1244 and timers_tests.rs:1083 use the same include pattern
- verifier: rust/src/main/registry.rs:520-610 scans `git ls-files --cached --others --exclude-standard`, requires exactly one active owner per path, applies the owner line limit to every `.rs`; new untracked part file under collab/** resolves to collab-runtime only
- file facts: ASCII, no tabs, no trailing whitespace, ends with newline

conclusion: split the trailing complete-test region lines 1268-1597 (330 lines, 7 whole `#[test]` fns) into collab/src/server/global_state_tests_part3.rs and replace it with `include!("global_state_tests_part3.rs");` at the original position. Main file becomes 1268 lines; part file 330 lines.
next: apply the mechanical split with apply_patch (one file at a time).

## node 2 - mechanical split (2026-10-07)

- moved region: original lines 1268-1597 (330 lines; 7 complete `#[test]` fns) -> `collab/src/server/global_state_tests_part3.rs`
- main file replaced that region with `include!("global_state_tests_part3.rs");` at the same position (after line 1267)
- exact move evidence: `sed -n '1268,1597p' <HEAD file>` SHA-256 `0b900fd5e19e80ffe5d757c6a30eaa39b658a7dcc4c49a03e945a0b5ead5910b` == part3 SHA-256; byte-identical
- splice equivalence: `(head -1267 main; cat part3)` SHA-256 `73b353654240766d79aef2c054d40750ec616c4f4cd22299c10418a5540a041a` == `git show HEAD:collab/src/server/global_state_tests.rs` SHA-256 -> no content lost or reflowed
- line counts: global_state_tests.rs 1597 -> 1268; global_state_tests_part3.rs 330
- `git diff --stat` for the tracked file: 1 insertion, 330 deletions
- no tabs, no trailing whitespace, no blank-line trimming in either file

deviation recorded: an initial `rustfmt --edition 2021` on the two owned files reformatted the ENTIRE main file (top-level test fns were re-indented from 4 to 0 spaces) because `collab/**` is not rustfmt-clean at baseline (`cargo fmt --manifest-path collab/Cargo.toml -- --check` already reports diffs in unrelated files at base). That mass reformat was reverted; the files were restored from `git show HEAD:` and the split reapplied. `collab/Cargo.toml` has no `[package] fmt` config and CI `verify.yml` runs `cargo fmt --check` only for `rust/` and `dagpipe/`, so collab is intentionally not fmt-enforced. No `rustfmt` was applied to the final files.
next: run the contract test command and the installed registry gate.

## node 3 - verification (2026-10-07)

command: `cargo test --manifest-path collab/Cargo.toml global_state -- --test-threads=4`
- result: `test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 888 filtered out; finished in 0.03s`
- log: /tmp/gs_test.log (full run captured); sha/size recorded by parent archive
- all 7 moved tests present and ok: an_equal_generation_address_change_is_still_rejected_for_a_live_incumbent, a_live_incumbent_route_still_tombstones_a_higher_generation_takeover, an_unregistered_project_incumbent_is_the_declared_non_resident_boundary, bind_runtime_still_rejects_a_lower_generation_over_a_live_binding, rebinding_an_address_after_tombstoning_drops_the_stale_tombstone, runtime_binding_ledger_replaces_stale_generation_for_the_same_binding, command_receipts_are_host_wide_and_idempotent
- no FAILED / panicked / error[ in the whole run
- compile clean (test binary built and linked)

test parity: `#[test]` counts main 24 + part2 7 + part3 7 = 38; base was main 31 + part2 7 = 38

command: `appsdk verify-sdk-source-registry .`
- result: stdout `{"ok":true,"gate":"sdk_source_registry"}`, exit code 0
- gate no longer reports collab/src/server/global_state_tests.rs
- out-of-scope note: gate did not report any other new violation, so no other owner-registration issue was observed under this candidate; `appsdk` also lists `collab/src/server/global_state_tests_part3.rs` as collab-runtime-owned and within limit (330 <= 1500)

working tree: only my scope changed -> `M collab/src/server/global_state_tests.rs`, `?? collab/src/server/global_state_tests_part3.rs`. No commit / push / install / restart / worktree was performed. No docs/collab.md or Rust CLI change by me.
next: report to parent; parent owns commit/review/merge/install and archives this run directory.
