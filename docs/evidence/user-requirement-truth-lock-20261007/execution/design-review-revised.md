# W4R Independent Design Re-Review

## Verdict

**DESIGN_PASS**

Scope: issue 592e241, base `c3c0c8df79e69534fe30c92db61328824473d5c0`.
Pre-coding design re-review of the revised candidate. No product code was
written and no design was edited. Long-term user authentication, physical
tamper resistance, and a durable version store remain explicitly `UNVERIFIED`;
that limitation is not used to reject the narrower template/context/review
binding increment.

## Prior Findings Disposition

### Prior P1 (stale-review admission deadlock) - RESOLVED

The revision separates a shared read-only author readiness helper from the full
admission gate. The design states the split and the source confirms it is real
and non-circular.

- `docs/design/appsdk-authoritative-review-packet.md:17` names
  `assert_pre_review_validation_gate` as the author candidate/validation
  evidence owner and records that `verify_review_admission` additionally checks
  existing publication/review, so it cannot be the new-context prerequisite.
- `appsdk-authoritative-review-packet.md:23` requires `review-context` to reuse
  the shared read-only readiness helper and forbids calling the full
  `verify_review_admission`.
- `appsdk-authoritative-review-packet.md:72` adds the `architecture_stable`
  stale-PASS recovery acceptance case.
- `rust/src/main/verification.rs:666` `assert_pre_review_validation_gate` reads
  only the fix-candidate and pre-review-validation records, checks candidate
  tree/source identity, rebuilds the artifact, and validates deployment and
  whitebox receipts. It reads no review or publication record.
- `rust/src/main/review_gates.rs:42-46` `verify_review_admission` is artifact
  match + preflight + `assert_pre_review_validation_gate` +
  `verify_internal(root, true, true, true, false)`.
- `rust/src/main/promotion.rs:1255-1269` is the `architecture_stable` review
  requirement, reached only through that `verify_internal` call inside the full
  admission path.

Consequence: after a requirement/template change on an `architecture_stable`
module, `review-context` can run the pre-review helper without the downstream
publication/review check. The circular recovery path is gone.

New PASS and author evidence are not weakened:

- `appsdk-authoritative-review-packet.md:23` and `:45` keep the full review gate
  for the new architecture PASS.
- `appsdk-authoritative-review-packet.md:70-71` keep missing, forged, or stale
  context rejection.
- `appsdk-authoritative-review-packet.md:72` keeps downstream rejection of the
  stale PASS and requires updated affected author evidence before a new context
  can be generated.

### Prior P2 (context field ambiguity) - RESOLVED

- `appsdk-authoritative-review-packet.md:41` freezes the binding: architecture
  PASS observation carries top-level `requirements_review: {context_id,
  checked:true}`; persistence is unique at
  `ReviewRecord.project_bindings.requirements_review`; `context_id` is returned
  at the public output top level; one AppSDK review-context owner shared
  recompute helper serves the context command, the architecture producer, and
  the later gate.
- `sdk-skill-sources/appsdk-project-governance/references/authoritative-review-template.md:77-78`
  names the exact public `review-context` output and the reviewer return.
- Binding is real: `rust/src/main/producer.rs:785-804`
  `lifecycle_chain_review_identity` includes `project_bindings`;
  `rust/src/main/lifecycle_closure.rs:516-526` computes `review_id` from it and
  `:548-550` persists `project_bindings`; `producer.rs:824-835` recomputes and
  rejects on mismatch. So a persisted `requirements_review` under
  `project_bindings` is covered by `review_id`.
- `appsdk-authoritative-review-packet.md:35` binds the context to
  requirement/candidate/template and excludes the mutable downstream stage, so
  a module's own review transition does not change its own context.

### Prior P2 (stale graph evidence) - RESOLVED

- `docs/evidence/user-requirement-truth-lock-20261007/graph-validation.txt:5-6`
  now reports `appsdk-user-requirement-consumption@0.2.0` (6 nodes, 5 edges);
  `:7-27` show the `assemble_review_packet` node, its edge, and the 6 waves.
- `appsdk-authoritative-review-packet.md:51` references the same graph file.

## Advisory (non-blocking)

- A1: The producer must read observation top-level `requirements_review` and
  merge it into `review.project_bindings.requirements_review` before computing
  `review_id`. The existing copy path
  (`rust/src/main/lifecycle_closure.rs:497-502,548-550`) copies
  `observation.project_bindings` only. Design `:41-43` implies the merge, but
  the implementation order should stay explicit so the acknowledgement is
  inside `review_id`.
- A2: The exact read-only prerequisite set for the shared helper
  (`appsdk-authoritative-review-packet.md:23`, "project/goal/resource valid") is
  deferred to the code owner. This is acceptable at design admission. Keep the
  helper limited to the pre-review read-only checks and do not pull in
  `verify_internal`.

## Authority Requirement Verification

Only an explicit user change may alter an effective requirement. The revised
candidate preserves the original user text, acceptance, scope, and
change-authority boundary, and it does not claim authentication, physical lock,
or user-authorized requirement mutation. No acceptance item was removed. The
AppSDK-owned template and distribution remain the owner of the review
procedure; the executing agent assembles facts and cannot authorize.

## Evidence

- Design: `docs/design/appsdk-authoritative-review-packet.md:17,23,35,41,43,45,51,67-72`.
- Template: `.../references/authoritative-review-template.md:74-86`.
- Source: `rust/src/main/verification.rs:666`; `rust/src/main/review_gates.rs:42-46`;
  `rust/src/main/promotion.rs:1255-1269`; `rust/src/main/producer.rs:785-804,824-835`;
  `rust/src/main/lifecycle_closure.rs:497-502,516-526,548-550`.
- Graph: `docs/evidence/user-requirement-truth-lock-20261007/graph-validation.txt:5-27`;
  `docs/dagpipe/user-requirement-consumption.graph.json:38-45,68-75`.
- Product Rust unchanged; static graph validate/inspect already PASS for `0.2.0`.
  Design review does not require future E2E.
