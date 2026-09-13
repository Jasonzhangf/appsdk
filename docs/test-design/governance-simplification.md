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

## Gate policy after ablation

The development path checks only the changed surface: formatting, focused tests,
type/check or build, and the declared contract. It does not require a freeze,
full regression suite, installation, restart, or live replay when those surfaces
are not part of the change.

`verify` and its `--admission` mode are read-only graph, integrity, and
freshness checks; they do not run external commands and accept a missing goal
record. A goal is still required and must be confirmed before a mutation,
compilation, promotion, review admission, or lifecycle producer writes
evidence.

Regression policy is stage-specific. A module may omit `regression` through
`draft`, `source_implemented`, `contract_bound`, `compiled`,
`controlled_verified`, and `architecture_stable`. A present declaration with
`required_before_freeze=false` is rejected at `architecture_stable`, preserving
the freeze boundary. A `frozen` or `retired` module must declare a valid
regression contract and publish a matching passing whitebox/blackbox report. If
a contract is present in an earlier stage, it is validated normally; malformed
declarations never become an omission.

Lifecycle producers reuse an existing PASS only when its recorded identity
still matches the current candidate/tree, artifact, scope, producer and
environment. The verifier also avoids rereading the same immutable JSON inputs
within one check. A changed input, dependency, artifact, environment, or
expired evidence invalidates only that phase and its downstream phases; the
immutable prior PASS remains a historical witness. Declared external commands
run again when their phase has no reusable evidence.

Historical governance records do not block new development by themselves.
Preserve/migrate keeps them immutable, while an explicitly authorized
`fresh_init`/reset creates a new epoch without copying old PASS records. Collab,
Guidance, Memory, and deployment operations block only work that actually
depends on them.
