# W11 Reviewer Finding Adjudication

## Scope

- Task: `requirements-g9-w11`
- Worker: `w11-migration-adjudication`
- Read-only adjudication at HEAD `93851a04e2fa40aaebab26b7b791fcf76a577c8a`.
- No tests, install, restart, worktree, or product-code changes run or created.

## Error Facts

1. The Codex P1 statement is stale on the schema shape: `requirements_review` is already declared under `project_bindings.properties.requirements_review`, with `context_id` and `checked` required inside that block. The container `project_bindings` itself is not globally required. Evidence: `contracts/records/review-record.schema.json:27-37`.
2. The formal finding is stale on the same point: it says the schema only defines an optional `project_bindings` container and does not declare `requirements_review`. Current schema declares it. Evidence: `.agent-collab/review/requirements-g9-codex-20261007-r1/review.final.md:3`, `contracts/records/review-record.schema.json:27-37`.
3. The design already forbids making hashes/checks into user authentication and forbids letting a producer fabricate reviewer acknowledgement after review. Evidence: `docs/design/appsdk-authoritative-review-packet.md:35`, `docs/design/appsdk-authoritative-review-packet.md:43`, `docs/design/appsdk-authoritative-review-packet.md:45`.
4. The design does not ask for a stage field in the review-record schema. It says bindings must not include current lifecycle stage, and compatibility is decided by historical publication semantics. Evidence: `docs/design/appsdk-authoritative-review-packet.md:37`, `docs/design/appsdk-authoritative-review-packet.md:47`.

## Evidence Gaps

1. The schema cannot alone prove that an upgraded modern `PASS` record retains the required binding. JSON Schema declares the allowed shape; it has no `verdict`/stage condition in the current schema lines. Evidence: `contracts/records/review-record.schema.json:5-37`.
2. The runtime gate is the owner of the modern `PASS` requirement. It reads `ReviewRecord.project_bindings.requirements_review` after checking the review identity, only when the module is not historical. Evidence: `rust/src/main/review_gates.rs:392-405`.
3. Historical compatibility is handled separately: frozen/retired modules skip the modern requirements-review binding check. Evidence: `rust/src/main/review_gates.rs:394-405`, `rust/src/main/producer.rs:839-911`.
4. The migration descriptor is a pinned snapshot migration for resource/function/mainline/verification maps. It does not transform review records or inject `requirements_review` into records. Evidence: `contracts/migrations/sdk-0.1.0010-to-0.1.0011.json:1-42`.

## Real Behavior Gaps

1. There is no implementation gap in the runtime modern/historical split at the cited gate: modern records must have the binding, historical frozen/retired records do not. Evidence: `rust/src/main/review_gates.rs:392-405`.
2. There is an evidence-consumer gap: a public pin-lock consumer black box should prove the upgraded path preserves the modern binding requirement and leaves historical records unchanged. The migration descriptor alone does not prove that behavior. Evidence: `contracts/migrations/sdk-0.1.0010-to-0.1.0011.json:6-8`, `rust/src/main/review_gates.rs:392-405`.
3. Do not add `requirements_review` to the schema's global `required` array. That would make every review record, including historical frozen/retired records and non-modern `FAIL`/`UNKNOWN` records, fail structural validation unless additional schema conditionals are added. The design explicitly allows `FAIL`/`UNKNOWN` to retain missing or unchecked reasons and keeps historical frozen/retired records on historical verification. Evidence: `contracts/records/review-record.schema.json:5-14`, `rust/src/main/review_gates.rs:394-405`, `docs/design/appsdk-authoritative-review-packet.md:45`, `docs/design/appsdk-authoritative-review-packet.md:47`.
4. `checked` remains a reviewer acknowledgement field, not user authentication. Hashes and schema validity identify review material; they do not prove the user authorized a requirement change. Evidence: `docs/design/appsdk-authoritative-review-packet.md:35`, `docs/design/appsdk-authoritative-review-packet.md:43`, `docs/design/appsdk-authoritative-review-packet.md:78`.

## Minimal Revision

Keep the schema declaration as is. Do not make `requirements_review` globally required. Add one public pin-lock consumer black box instead:

1. Build a modern architecture `PASS` fixture for an upgraded consumer after pin-lock migration.
2. Assert a PASS missing `project_bindings.requirements_review` is rejected.
3. Assert a PASS with the current public `context_id` and `checked: true` is accepted by the relevant consumer/gate.
4. Assert a historical frozen/retired record remains unchanged and continues through the historical path.

This proves the behavior split without asking migration to invent reviewer acknowledgement and without collapsing modern PASS requirements into historical record compatibility.
