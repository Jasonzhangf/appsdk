Contract complete. Final state verified:

**Result**
- Blocker removed: gate went from `SDK_SOURCE_LINE_LIMIT:collab/src/server/global_state_tests.rs:1597>1500` to `{"ok":true,"gate":"sdk_source_registry"}` (exit 0).
- Files: `collab/src/server/global_state_tests.rs` 1597 → 1268 lines; new `collab/src/server/global_state_tests_part3.rs` 330 lines. Diff is exactly 1 insertion + 330 deletions.
- Owner unchanged: `collab-runtime`, `owned_paths ["collab/**"]`, default 1500; the new part file resolves to that same owner.
- Test parity: 24 + 7 + 7 = 38 `#[test]` fns, equal to base 31 + 7.

**Mechanical proof**
- `sed -n '1268,1597p' <HEAD file>` SHA-256 `0b900fd5…b5910b` == part3 SHA-256 → byte-identical move.
- `(head -1267 main; cat part3)` SHA-256 `73b3536…041a` == `HEAD:collab/src/server/global_state_tests.rs` → nothing lost, no blank-line trimming.
- Followed the existing `include!("global_state_tests_part2.rs")` convention at the same position.

**Verification**
- `cargo test --manifest-path collab/Cargo.toml global_state -- --test-threads=4` → `ok. 38 passed; 0 failed; 888 filtered out`, exit 0; all 7 moved tests `... ok`.
- `appsdk verify-sdk-source-registry .` → `{"ok":true,"gate":"sdk_source_registry"}`, exit 0.

**Notes**
- I initially ran `rustfmt` on the two owned files; it mass-reindented the whole main file because `collab/**` is not fmt-enforced or fmt-clean at baseline. I reverted it by restoring exact HEAD bytes and reapplying the split. No `rustfmt` was applied to the final files.
- No other gate violation appeared, so nothing outside my scope was touched. No commit/push/install/restart/worktree.
- Records: `notes.md` (nodes + evidence) and `report.md` in `/Volumes/Intel/playground/appsdk/.worker-runs/requirements-session-lock/baseline`. Ready for parent to commit/review/merge and then archive the run directory.