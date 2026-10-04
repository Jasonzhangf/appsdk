# a7cc2a4 pin composition run notes

## 2026-10-03 | baseline | confirmed

- Conclusion: assigned worktree is clean at `61e8c534b5295fb0f60a6634ad3cf99d99f12a4c` on `codex/a7cc2a4-pin-compose-20261003`.
- Evidence: `git status --porcelain=v1`, `git rev-parse HEAD`, `git branch --show-current`, `git worktree list --porcelain`.
- Inputs: global AGENTS; `coding-principals`; diagnosis report and graph; current `main` includes `dfa7918` static registry repair.
- Next: verify the frozen source archive and inspect every old-candidate source delta.

## 2026-10-03 | frozen source input | confirmed

- Conclusion: the previous candidate archive is byte-identical to the declared genuine fixture hash `2d91c66612c307e2ba76ceba188ca613e557840d69b9ef1568bb33825e68db50`; the old staged tree is `93bec4fd39dcc1d3fab588e2830c622102f908c1` over base `bb9168d86342c7cfe910e1124b25e6bb7a8cffd9`.
- Evidence: `shasum -a 256 /Volumes/Intel/playground/appsdk/a7cc2a4-pin-impl-20261003/docs/evidence/a7cc2a4-pin-impl-20261003/fixture-agentteams-de099b79.tar.gz`.
- Frozen product blob hashes: `migration.rs d266fd578be85616ad2579a7e1c90afef4359d8a95d2dee67284b42c0f097936`; `sdk-0.1.5-to-0.1.6.json 8a6ecdfcae0d23f4c07b800a16ff80d287e83905eeeb3dcdc7a006036755e626`; `part_08.rs ff15dce770bf694fd36574b0ebb2946636f50b710ee9203a4d0d59130b388ebd`; `manifest.json aa00bcc953a8716398fa592712fdb49450f36de1cf9d8dfc6cae4038bf0136cc`; `dagpipe.rs 61c6839bf9eb32294903d63769947ce35087765a5a62d32c22aa900cd784499f`.
- Next: export the exact old product delta and record the composition root cause.

## 2026-10-03 | source delta and composition | confirmed

- Conclusion: the unique admitted repair is the pair-bound `historical_target_digests` authorization in `assert_sdk_migration_record`, its four frozen tuples, and the historical public-CLI tests. The old candidate also added an evidence archive and registered the graph.
- Composition root cause: the old candidate registered `sdk-pin-history` against a 7-graph/5-design-ID/27-operator base. Current `main` already contains `dfa7918` with 9 embedded graphs, 7 design IDs, and 41 operators. The composed static registry owner must be 10 embedded graphs, 8 design IDs, and 47 operators; the old arrays cannot be restored.
- Excluded source: no root-transition refresh helper, runtime contract, `part_09.rs`, `producer.rs`, `reset_governance.rs`, Teams code, or transition fixture is part of the admitted delta.
- Next: build the unmodified base CLI and replay the complete frozen fixture as the required red.

## 2026-10-03 | evidence command failures | failed and corrected

- Failure 1: the first base-red shell wrapper used zsh's read-only `status` variable after `pin-lock`; the CLI produced the expected `INVALID_SDK_MIGRATION_RECORD`, but the wrapper exited before recording the exit code. Re-ran with `rc`; exit `1` was then captured.
- Failure 2: the first full-archive wrapper used `path` as a `while read` variable, clobbering zsh's `PATH`; the candidate `pin-lock` did not run and the command exited `127`. The extracted fixture had no current-step record and the registry remained empty. Re-ran with `rel`; candidate `pin-lock` then exited `0`.
- Evidence: `logs/base-pin-lock.stderr`, `logs/candidate-pin-lock.stdout`, `logs/candidate-verify.stderr`.
- Source delta fingerprint: `docs/evidence/a7cc2a4-pin-compose-20261003/source-delta.patch` SHA-256 `d7b57ea30112fa14680b405a368d90a21626d441816e7995e1b9b23a546f1302`.

## 2026-10-03 | base red | confirmed

- Conclusion: the unmodified `61e8c53` CLI built successfully and rejected the complete unchanged fixture with exit `1`, empty stdout, and stderr `INVALID_SDK_MIGRATION_RECORD`; before/after fixture hash trees are identical and the task-owned registry remained empty.
- Evidence: `logs/base-build.log`, `logs/base-pin-lock.stdout`, `logs/base-pin-lock.stderr`, `logs/base-fixture-before.sha256`, `logs/base-fixture-after.sha256`.
- Inputs: base binary SHA-256 `39700c5b08402b16ccbb2e66bbf98184a4da43ccfc2bf09112bd1437e593f328`; fixture archive SHA-256 `2d91c66612c307e2ba76ceba188ca613e557840d69b9ef1568bb33825e68db50`.
- Next: apply the unique historical tuple repair and compose the DAG registration.

## 2026-10-03 | implementation and focused tests | completed

- Conclusion: the exact frozen validator/tuple/part_08 sources are byte-identical to the old staged blobs. `dagpipe.rs` and the manifest retain `dfa7918` and add `sdk-pin-history` at the composed 10/8/47 counts.
- Evidence source hashes: `migration.rs d266fd57...`, `sdk-0.1.5-to-0.1.6.json 8a6ecdfc...`, `part_08.rs ff15dce7...`, `manifest.json aa00bcc9...`.
- Focused tests: complete historical baseline PASS; historical custom target PASS; preserved historical custom maps PASS; `pin_lock_rejects` 15 passed, 0 failed; manifest-driven DAG registry test PASS.
- Evidence: `logs/test-complete-historical.log`, `logs/test-historical-custom-target.log`, `logs/test-historical-custom-maps.log`, `logs/test-pin-lock-rejects.log`, `logs/test-dagpipe-manifest-registry.log`.
- Next: rebuild and verify public DAG enumeration plus the full archive.

## 2026-10-03 | public DAG and full archive | completed with dependency

- Conclusion: `dagpipe graph validate` passed; rebuilt candidate CLI outside the SDK cwd emitted 10 unique IDs matching the 10-entry manifest exactly, including `appsdk-sdk-pin-history`. Candidate `pin-lock` on the complete unchanged archive exited `0`; the original historical record, four snapshots, four live maps, and module registry remained byte-identical; the generated current-step record's source, target, and live digests agree per map while canonical target provenance is retained.
- Dependency: ordinary `verify` exited `1` with `INVALID_DECLARED_ZONE_CONTRACT`. Current standalone `main` lacks the `1a21b0c` root-refresh repair, so this is an explicit remaining dependency, not an admitted product scope. Full admission is not claimed.
- Evidence: `logs/dagpipe-graph-validate.log`, `logs/candidate-dagpipe-validate.json`, `logs/candidate-dagpipe-validate.ids`, `logs/manifest.ids`, `logs/candidate-pin-lock.stdout`, `logs/candidate-historical.diff`, `logs/candidate-current-step-live-agreement.tsv`, `logs/candidate-verify.stderr`, `logs/candidate-build.log`, `logs/candidate-fmt-check.log`.
- Inputs: candidate binary SHA-256 `da7cfdefa7c68ab342ae9858bd7515a12a7d93869b57847bb87673d8068ea558`; fixture archive unchanged at `2d91c666...`.
- Next: final allowlist, fingerprint, and cleanup receipt.

## 2026-10-03 | final scope and cleanup | completed

- Conclusion: tracked product changes are exactly the five allowed paths; evidence is confined to `docs/evidence/a7cc2a4-pin-compose-20261003/**` plus the unchanged archive at its original `docs/evidence/a7cc2a4-pin-impl-20261003/**` path. `contracts/sdk-bundle.manifest.json` is unchanged because no official producer was used. `git diff --check` and `cargo fmt --check` passed.
- Retained for parent: candidate CLI `.tmp/candidate/appsdk`, complete archive, source delta, logs, and fingerprints. The author first copied the CLI to `docs/evidence/a7cc2a4-pin-compose-20261003/candidate-appsdk`; after consuming exit0/turn.completed and verifying its hash, primary moved those identical bytes to the task-owned temporary candidate directory. Original copy/hash output and author cleanup receipt retain the former path as historical evidence; the binary is not part of the staged Git change. `.tmp/` remains an explicitly retained untracked resource until the dependency replay no longer needs that binary.
- Removed after receipt: task-owned target cache, extracted fixtures, registries, temporary directories, and outside-cwd scratch. The original frozen candidate worktree and resources were not touched.
- Status: candidate awaiting dependency/review; not merged, fixed, installed, or released.

## Primary terminal consumption

- Author11221 is ESRCH, wrapper exit0 and final turn.completed consumed. All five product hashes plus the unchanged fixture archive in logs/candidate-source.sha256 match. Retained binary hash remains `da7cfdefa7c68ab342ae9858bd7515a12a7d93869b57847bb87673d8068ea558`.
- Scope/source tests and actual 10-ID registry equality are accepted as current author facts. Full ordinary verify remains blocked by separate1a21b0c; no independent architecture review, source integration, installation or Teams admission is claimed.
- Only this note and the retained temporary binary location changed during consumption. Product/test/fixture fingerprints remain unchanged; their applicable successful evidence is reused.
- After staging, five raw test logs and source-delta.patch produced whitespace failures in `git diff --cached --check` that the author's unstaged-source check did not cover. Primary preserved each exact original byte stream as its `.gz` counterpart, verified decompress equality and recorded original/compressed hashes in lossless-log-compression-receipt.json. References to those original raw filenames above describe the author invocation; their current evidence files have the `.gz` suffix. No output or test result was rewritten and no gate was bypassed.

## 2026-10-03 | current source-gate correction | in progress

- Conclusion: parent source gate identified `part_08.rs` at 1742 lines against the 1500-line test-source limit. The candidate added 245 top-level lines immediately after base line 394; only those existing helpers/constants/tests are moved, with no product source, pin validator, manifest, schema, registry, or `part_20.rs` change.
- Evidence: exact staged diff showed one `include_bytes!` constant, six helper functions, and eight tests; historical author receipts above remain preserved as provenance.
- Inputs: base/candidate `61e8c534b5295fb0f60a6634ad3cf99d99f12a4c`, branch `codex/a7cc2a4-pin-compose-20261003`, admitted `appsdk-sdk-pin-history` graph, and task-owned `CARGO_TARGET_DIR`, `APPSDK_HOME`, and `TMPDIR` under `.source-gate-current`.
- Next: verify formatting, execute the eight moved tests with 15 rejects through the real CLI target, rebuild the candidate binary, run source registry on an exact tracked-source snapshot, bind current hashes, and clean task-owned caches.

## 2026-10-03 | current source-gate correction | bounded result

- Conclusion: moved the exact existing candidate helper/test block from `part_08.rs` to new `part_21.rs` and added its include. `part_08.rs` returns to 1497 lines, `part_21.rs` is 244 lines, and `main.rs` is 22 lines; both changed Rust files are <=1500. The block retained its exact names, include fixture path, and behavior.
- Current tree/binary: tracked candidate tree `fe5dd452eb2105f0a31fffb9bff435e31625021f`; clean archive hash `386dffc44640de210e3e2f7fd4e87b29fee00733ad75539cc437f62e780be8cc`; rebuilt binary hash `9e15b57593b6318d493aaf853f73692b4095e9b3a4d34fde58ef83a4a613060c`. The tracked tree and worktree agree exactly for `main.rs`, `part_08.rs`, and `part_21.rs`.
- Product binding preserved: `migration.rs d266fd578be85616ad2579a7e1c90afef4359d8a95d2dee67284b42c0f097936`; `sdk-0.1.5-to-0.1.6.json 8a6ecdfcae0d23f4c07b800a16ff80d287e83905eeeb3dcdc7a006036755e626`; `manifest.json aa00bcc953a8716398fa592712fdb49450f36de1cf9d8dfc6cae4038bf0136cc`; `dagpipe.rs 6761008bbe142f4db6089ce8494eca6d6176f14f8315896264c7a974ba387c08`; fixture archive `2d91c66612c307e2ba76ceba188ca613e557840d69b9ef1568bb33825e68db50`. No pin validator, manifest/schema, registry, manifest ID, or `part_20.rs` logic changed.
- Focused real-CLI results: `cargo fmt --check` PASS; complete historical baseline exact test PASS 1/1; historical custom target exact test PASS 1/1; historical custom maps exact test PASS 1/1; `pin_lock_rejects` PASS 15/15 (zero failures); broad `pin_lock_` PASS 27/27 (zero failures). Actual names and counts are in `.source-gate-current/logs/`; they are author evidence, not independent review.
- DAG validation: exact graph validates as `appsdk-sdk-pin-history@0.1.0` with 6 nodes, 5 edges, and 6 waves; snapshot `dagpipe validate` PASS and emits 10 IDs exactly matching `docs/dagpipe/manifest.json`.
- Source registry result: the outside-snapshot executable ran `verify-sdk-source-registry` against the clean exact tree snapshot and exited 1 at pre-existing `collab/src/identity.rs:1826>1500`. That path is byte-identical and unchanged from base 61e8c53, so it is not attributable to this test move and was neither ignored nor bypassed. Full acceptance remains blocked until that separate source-size issue and the ordinary `1a21b0c` dependency are resolved/composed.
- Scope/cleanup: current exact edits are staged at only `rust/tests/cli_smoke/part_08.rs`, `rust/tests/cli_smoke/part_21.rs`, `rust/tests/cli_smoke/main.rs`, and this evidence note; historical staged files remain parent-provided provenance. No commit, review, merge, push, install, release, Collab, AGY, agent, fork, or memory work was performed. Retained existing executable `.tmp/candidate/appsdk` and its prior hash remain untouched; task-owned build cache and snapshot were removed after retaining the current executable once at `.tmp/candidate/source-gate-current/appsdk` with its SHA-256 receipt. `.tmp` is now 70M: the pre-existing candidate binary plus the one current executable and no build outputs.
- Current binding status: test/source-shape correction complete; full source architecture gate is `INCOMPLETE` due the unrelated base `collab/src/identity.rs` limit, not `PASS`.

## Primary dependency and resource receipt / 2026-10-04T02:53:35.550316+00:00

The complete unchanged historical Teams archive passes first pin/ordinary verify and reentry with preserved historical bytes in the separate exact pin+transition production composition. composed-archive-acceptance.tar.gz and composed-archive-primary-receipt.json retain the full evidence/source binding; this is not a mixed delivery commit or installed acceptance. Strict source registry remains incomplete on unchanged base identity.rs1826>1500 (upstream f7bf558).

R2 current tested executable is now retained outside Git at /Users/fanzhang/.codex/task-evidence/agentteams/receipts/a7cc2a4-pin-compose-20261003/r2-primary-retained/appsdk, SHA2569e15b57593b6318d493aaf853f73692b4095e9b3a4d34fde58ef83a4a613060c. All current raw logs copied byte-equal to /Users/fanzhang/.codex/task-evidence/agentteams/receipts/a7cc2a4-pin-compose-20261003/r2-primary-retained/source-gate-logs; prior in-tree paths are historical provenance. The superseded compiled binary da7cfdef and owned .tmp directory may be reclaimed; current product fingerprints remain unchanged by this evidence relocation. Architecture review, integration/push/install pending.
