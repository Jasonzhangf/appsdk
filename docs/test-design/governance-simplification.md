# Governance simplification acceptance

## Scope

AppSDK quality/safety/evidence integrity remains mandatory when applicable.
Automatic Collab peer bootstrap, messaging and multi-worker task/file ownership
remain enabled. Failures block dependent shared operations, not independent
quality checks. Guidance and Memory are auxiliary.

The SDK worktree owns its Rust gates, schemas, maps, templates and bundled Skill.
Global policy owners are `~/.agents/AGENTS.md`, the shared review standards,
Collab/Memory/coding/alignment Skills and the agent-routing references. They are
not copied into consumer projects. Their before/after diff is saved for this
review in `.appsdk-control/global-rules.patch`; originals are retained in
`.appsdk-control/governance-simplify-backup/`.

## Behavioral regressions

- Collab automatically runs with the inherited peer environment; success,
  process failure and unavailable executable are distinct. No fabricated peer.
- Collaboration-only activation succeeds. A selected merge queue still requires
  ownership, tested integration and remote-main receipts.
- Advisory Guidance status/init never reports a mandatory flow. Ordinary
  verification works without a plan or memory. Close does not require unrelated
  freeze or cleanup. Selected workflows keep their history and drift checks.
- Module deployment operations can require both receipts, one or neither.
  Missing required blackbox, forged/late receipts and artifact drift still fail.
  Applicability is artifact-bound; every supplied receipt is checked.
- Review passes through the full lifecycle without a confidence score.
- Valid pre-review intervention/blackbox evidence can be reused; input identity,
  artifact identity, timestamps, expiry and source-change checks remain binding.
- Freeze/publish/rehydrate, migration and negative evidence tests remain green.

## Verification entrypoints

`cargo test --manifest-path rust/Cargo.toml`, `cargo fmt --check`, release build,
`appsdk verify-sdk-source-registry .`, isolated installed CLI new/init/verify/
Guidance smoke, JSON schema parse and Skill validation. Review prompt changes
also run the existing DSH MCP contract tests and Node syntax checks.

The SDK is a CLI, so service restart is not applicable. Shared Collab daemon
replacement is outside this change; the existing daemon remains its own owner.
No business project migration, remote merge or release is implied by this test.
