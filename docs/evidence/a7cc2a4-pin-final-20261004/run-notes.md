# a7cc2a4 pin-final author verification

## Scope and binding

- Issue: `a7cc2a4`.
- Branch: `codex/a7cc2a4-pin-final-20261004`.
- Base/HEAD before verification: `7a23f539610cec728ec983619c6b10c7f10055d7`.
- Candidate product tree (staged before this new evidence): `98b94df3431e495456fdd5fb41b381bc71191668`.
- Candidate binary: `/Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache/target/debug/appsdk`.
- Candidate binary SHA-256: `b12b6e796e5ef168e28ccbc51947aed84c7a74b934d90a72539628f43086a7ec`.
- Retained tested executable: `$HOME/.codex/task-evidence/agentteams/receipts/a7cc2a4-pin-final-20261004/retained/appsdk`; byte-identical copy verified with `cmp`, same SHA-256.
- External owned Cargo cache: `/Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache`.
- Product/test/map/registry files were not edited by this verification. The exact frozen product hashes match the prior composition receipt:
  - `rust/src/main/migration.rs d266fd578be85616ad2579a7e1c90afef4359d8a95d2dee67284b42c0f097936`
  - `contracts/migrations/sdk-0.1.5-to-0.1.6.json 8a6ecdfcae0d23f4c07b800a16ff80d287e83905eeeb3dcdc7a006036755e626`
  - `docs/dagpipe/manifest.json aa00bcc953a8716398fa592712fdb49450f36de1cf9d8dfc6cae4038bf0136cc`
  - `rust/src/dagpipe.rs 6761008bbe142f4db6089ce8494eca6d6176f14f8315896264c7a974ba387c08`
  - `docs/evidence/a7cc2a4-pin-impl-20261003/fixture-agentteams-de099b79.tar.gz 2d91c66612c307e2ba76ceba188ca613e557840d69b9ef1568bb33825e68db50`
- The normalized frozen-product hash comparison exited `0`; the first comparison differed only in `path hash` versus `hash  path` formatting and is retained as `logs/frozen-product-diff.formatting.txt`.

## Commands and results

1. `CARGO_TARGET_DIR=/Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache/target cargo fmt --manifest-path rust/Cargo.toml -- --check`
   - Exit `0`; raw output is empty in `logs/cargo-fmt-check.log`; exit receipt is `logs/cargo-fmt-check.exit`.

2. `CARGO_TARGET_DIR=/Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache/target cargo build --manifest-path rust/Cargo.toml --bin appsdk`
   - Exit `0`; raw output is `logs/cargo-build.log`; exit receipt is `logs/cargo-build.exit`.

3. `CARGO_TARGET_DIR=/Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache/target cargo test --manifest-path rust/Cargo.toml --test cli_smoke pin_lock_ -- --nocapture`
   - Exit `0`; losslessly compressed raw output is `logs/cargo-test-pin-lock.log.gz`; exit receipt is `logs/cargo-test-pin-lock.exit`.
   - Result: `running 27 tests`; `test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 301 filtered out`.
   - Nonzero count and expected 27 are recorded in `logs/test-count-check.txt`.
   - Compression binding: raw SHA-256 `9f7efd508e6e8518de62adfab6a673b4585c7416c14908d363fb19e714fc5078`; compressed SHA-256 `a5f65157761fb994eef8d1d839fb67f0abdd25e38de354a98132ff3216e53bbf`; decompression equality is recorded in `logs/lossless-compression-check.exit`.

4. From outside-source cwd `/Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache`:
   `/Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache/target/debug/appsdk verify-sdk-source-registry /Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004`
   - Exit `0`; stdout `{"ok":true,"gate":"sdk_source_registry"}` in `logs/source-registry.stdout`; stderr empty in `logs/source-registry.stderr`; exit receipt is `logs/source-registry.exit`.

5. From outside-source cwd `/Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache`:
   `/Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache/target/debug/appsdk dagpipe validate`
   - Exit `0`; JSON is `logs/appsdk-dagpipe-validate.json`; stderr empty in `logs/appsdk-dagpipe-validate.stderr`.
   - Extracted IDs are `logs/appsdk-dagpipe-validate.ids`; manifest IDs are `logs/manifest.ids`; both contain exactly 10 IDs and compare equal with `cmp` exit `0`.
   - IDs: `appsdk-fix-lifecycle`, `appsdk-notification-object`, `appsdk-collab-context`, `appsdk-collab-appserver-route-repair`, `appsdk-collab-subscription-lifecycle`, `appsdk-collab-notification-consumption`, `appsdk-collab-merge-pending`, `appsdk-collab-identity-adjudication`, `appsdk-collab-dsh-channel`, `appsdk-sdk-pin-history`.

6. Discovered actual graph path before validation: `docs/dagpipe/sdk-pin-history.graph.json` (the hypothetical `docs/dagpipe/graphs/sdk-pin-history.graph.json` path does not exist).
   `dagpipe graph validate docs/dagpipe/sdk-pin-history.graph.json`
   - Exit `0`; raw stdout `valid DAG: appsdk-sdk-pin-history@0.1.0 (6 nodes, 5 edges, 6 waves)` plus the operator-binding notice in `logs/dagpipe-graph-validate.stdout`; stderr empty in `logs/dagpipe-graph-validate.stderr`.

7. `git diff --cached --check`
   - Exit `0`; stdout and stderr are empty in `logs/staged-diff-check.stdout` and `logs/staged-diff-check.stderr`.

## Boundaries

- Existing composed-archive pin+transition evidence remains the applicable archive evidence; the unchanged full archive was not rerun.
- Ordinary `verify` still depends on separate `1a21b0c` for root-refresh repair. This pin-only candidate was not installed, did not mutate canonical runtime/identity/service state, and does not claim full consumer closure.
- No Collab, AGY, other workers, model catalog admission, resume/fork, global installation, commit, merge, or push was used.
- Owned external Cargo cache was removed after evidence capture; `test ! -e /Volumes/Intel/playground/appsdk/a7cc2a4-pin-final-20261004-cache` passes. The retained executable is outside Git and is the required verification artifact. The source worktree and all prior resources remain untouched.
- The raw `cargo test` log was gzip-compressed only because its trailing blank line caused `git diff --cached --check` to fail; the decompressed bytes match the pre-compression SHA-256 exactly.

## Cleanup receipt

- `cleanup-receipt.json` records the owned cache removal and retained-binary verification.
- No owned fixtures or temporary directories remain outside the allowed evidence path and the retained binary path.

## Primary security finding correction (1d16a28)

2026-10-04T05:23:49.931251+00:00 / independent r1 FAIL P0: author captured full environment in staged environment.txt. Candidate never committed/pushed. File removed from index/worktree; replaced by environment-summary.json with explicit non-secret allowlist. Own duplicated evidence/review logs redacted, originals isolated0700/0600 and referenced only by external security receipts. These redactions are not lossless raw evidence. Source/fixture fingerprints and all test outputs unchanged, so valid build/pin27/source/registry evidence reused. Current staged candidate scanned against original captured credential values (no values output), zero matches. Shared Git object store retains an uncommitted local blob; no unsafe prune or account rotation performed. Credential/account risk remains externally retained under issue1d16a28, no false closure. Next exact independent review of sanitized pin candidate.
