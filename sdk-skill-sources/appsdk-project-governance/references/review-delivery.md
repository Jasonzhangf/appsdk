# Review and Delivery

Ordinary work uses relevant diffs, behavior results and normal Git delivery.
The release boundary and prohibited checks are owned by
[the governance Skill](../SKILL.md#purpose-and-mandatory-boundary).

## Authoritative requirement review packet

Every design or architecture review that consumes project requirements uses
[authoritative-review-template.md](authoritative-review-template.md). The
dispatch must provide or clearly reference:

- the project's authoritative requirement source, owner, exact read path, and
  effective version;
- the user original text, acceptance criteria, applicable scope, and any
  explicit user change instruction with its prior version;
- the work branch or diff, base, review stage, allowed/forbidden paths, and current
  stage evidence; and
- the existing behavior results and backend-supplied review output schema.

The executing agent fills only observed project facts and evidence references.
It must not replace the source with a plan or author summary, edit the template
for one task, or create an authorization. The independent reviewer reads the
source and current version, compares the prior version when a change is
claimed, and records each applicable item-to-design, item-to-implementation,
and item-to-evidence mapping in the existing backend fields. Missing required
sources, stale versions, unauthorized requirement changes, reduced acceptance,
and candidate/scope mismatches block the review. Do not create a second review
schema or a second requirement store.

## Ordinary review and delivery

```text
diff and affected public-entrypoint behavior checks
-> applicable independent milestone review
-> combine with latest mainline
-> protected merge/push and Git receipt
-> release owned resources
```

Follow the host contract for review timing and backend choice. One complete,
significant milestone receives one independent review after author validation
at its integration boundary. Internal nodes and ordinary commits do not
trigger another review. Documents and Skills use targeted checks.

Install or restart only when the affected behavior requires that operation.
Use its ordinary receipt and observe the affected public entrypoint. A library
uses a real consumer. Whitebox checks help locate relevant failures; do not add
them as a second mandatory admission layer. Do not add binary or configuration
hashes to these results.

Changed-scope architecture review must verify:

- control/configuration truth uses declared typed control resources, error
  chains, or project configuration sources and never business payloads,
  metadata, debug logs, or implicit context;
- each semantic behavior has one owner and one implementation, with no fallback
  or temporary bypass;
- a project-declared lifecycle skeleton preserves its owning boundaries;
- additions passed an ablation check and common semantics use one shared
  function;
- missing operators, hooks, or gates fail or skip explicitly and never mock
  success.

Concrete quality, safety, contract or material structural regressions block.
Optional simplification and design preferences are advisory. Untouched
historical violations are reported as recommendations and do not block unless
they affect changed scope, safety, ownership, evidence truth, or required
delivery.

Material changes to behavior, dependencies or the tested environment reopen
only affected checks. A new phase, log entry or Git identifier does not by
itself require a rerun or review. Reuse valid results directly;
revise the plan only when goal, scope, acceptance, key approach, or dependencies
materially change. Follow the host contract for review triggers and finding
re-checks; internal progress does not dispatch another review.

Conflict returns to the owner worktree. Failed and unknown results stay visible.

## Existing publication-record implementation

The current runtime still implements `PreReviewValidationRecord`,
`EffectivenessRecord` and the hash-bound lifecycle graph. Commands such as
`produce-lifecycle-chain`, `verify --review-admission`, promotion and freeze
consume those records. A procedure edit does not remove their program checks.

Do not add this record chain to ordinary work. If a requested operation still
depends on it before release preparation, identify and change the enforcing
owner; do not fabricate PASS, edit records or suppress the error. Current
implementation details are in the SDK source at `docs/design/fix-lifecycle-v2.md`
and `docs/design/lifecycle-records.md`.

External release preparation may use promotion, Active/Protected and freeze
only when the actual distribution requires them. Select necessary final
artifact and source checks once. Existing fields, historical contracts and
available commands do not justify a full protection suite. Published history
remains unchanged.

## Cleanup

Preserve needed evidence and remove only owned resources after delivery. Use
the removal result to close the cleanup obligation. Do not create archive
hashes, frozen patches or byte-comparison manifests. Preserve dirty or active
resources when removal would lose work; report the path, owner, reason and next
action. Delivery and resource closure remain distinct results.
