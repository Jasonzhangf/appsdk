# W4 Independent Design Review

## Verdict

**DESIGN_FAIL**

Scope: issue 592e241, base `c3c0c8df79e69534fe30c92db61328824473d5c0`.
This is a pre-coding design review. Product Rust was not changed. Long-term
user authentication and physical tamper resistance remain explicitly
`UNVERIFIED`, and that limitation was not used to reject the narrower
template/material/version-binding increment.

## Authority Requirement Verification

The dispatch and `docs/design/user-requirement-truth-lock.md` establish the
current requirement: only an explicit user change may modify an effective
requirement; the AppSDK governance owner must own the formal review template
and distribution; an executing agent may assemble observed project materials
but may not create authorization or replace the authoritative source.

The candidate correctly separates this increment from the unimplemented
long-term requirement lock. It preserves the original user text, acceptance,
scope, and change-authority boundary. It does not claim authentication,
physical tamper protection, or user-authorized requirement mutation.

## Blocking Finding

### P1: `review-context` has a circular dependency after a stale review

Evidence:

- `docs/design/appsdk-authoritative-review-packet.md:17` identifies
  `verify_review_admission` as the pre-review author-verification gate.
- `docs/design/appsdk-authoritative-review-packet.md:23` requires
  `appsdk review-context` to pass that gate before it can emit context.
- `rust/src/main/review_gates.rs:42-46` invokes `verify_internal` with module
  publication checks enabled.
- `rust/src/main/promotion.rs:1255-1269` then requires the current
  architecture review record for an `architecture_stable` module.
- The same design requires a stale requirement/template change to invalidate
  the old review PASS (`appsdk-authoritative-review-packet.md:43-45,70-71`).

Failure condition: after a requirement or template change on an
`architecture_stable` module, the old review is stale and must be rejected.
However, generating the new review context requires admission, and admission
requires the old review to be current. The system cannot obtain the new
context needed to replace the stale review. This is a real recovery deadlock,
not a cosmetic naming issue.

Minimal revision: define separate owners for pre-review readiness and
post-review admission. `review-context` must call the pre-review
candidate/validation/evidence checks (the existing
`assert_pre_review_validation_gate` path and its read-only prerequisites), not
the full `verify_review_admission`. Keep `verify_review_admission` as the
post-review gate. Add a black-box recovery case: stale old PASS on an
`architecture_stable` module -> new `review-context` succeeds -> old PASS is
rejected -> new context-bound review PASS is accepted. Frozen/retired
historical publications remain on their existing historical validation path.

## Non-Blocking Findings

### P2: Persisted graph evidence is stale

The current `docs/dagpipe/user-requirement-consumption.graph.json` is version
`0.2.0` with 6 nodes and 5 edges. `dagpipe graph validate` and `inspect` PASS
for that current file. The persisted evidence in
`docs/evidence/user-requirement-truth-lock-20261007/graph-validation.txt:9-11`
still reports version `0.1.0`, 5 nodes, and 4 edges, and the stage notes repeat
that stale result. Update the evidence and notes to the current `0.2.0`
6-node graph so the design handoff does not point implementers at an older
topology.

### P2: Freeze the `context_id` return and binding location

The design requires `requirements_review.context_id`, but it defers the exact
field placement and the return path to the code owner
(`appsdk-authoritative-review-packet.md:41`). The template packet also has no
explicit `context_id` field. Freeze one existing AppSDK-owned location for the
reviewer-returned context ID and one shared recomputation helper used by the
context producer, review identity, and post-review gate. This prevents two
independent bindings from drifting while preserving the existing schema.

## Verification Evidence

- Current graph validation: `appsdk-user-requirement-consumption@0.2.0`,
  6 nodes, 5 edges, PASS.
- Current graph inspection: single source, single sink, serial dependency
  waves, PASS.
- `git diff --check`: PASS.
- Product Rust: unchanged in this increment.
- Long-term authentication, physical write isolation, and model compliance:
  still `UNVERIFIED`, as the design states.

## Required Revision Before Re-Review

Revise the design to separate the pre-review context prerequisite from the
post-review admission gate, add the stale-review recovery acceptance case,
refresh the stale graph evidence, and name the single `context_id` binding
owner. No product code should be written until that revision receives a new
independent design PASS.
