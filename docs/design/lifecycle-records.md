# Lifecycle Record Contract

String requirements are only declarations. A closed lifecycle requires records with evidence, producer, scope, identity, and freshness.

## Records

- `GoalClarificationRecord`: raw request, restated objective, acceptance criteria, non-goals, assumptions, ambiguities, questions, scope, confirmation, and admission status;
- `EvidenceRecord`: one red/positive/negative test, replay, build, artifact, runtime, or gate result;
- `ReviewRecord`: reviewer identity, reviewed commit, verdict, evidence IDs, and AI confidence rationale;
- `PromotionRecord`: issue/experiment, base/source commits, old/new Active versions, hashes, review, gates, compatibility, and migration;
- `RegressionReport`: freeze candidate's whitebox and blackbox regression result bound to source, scope, artifact, API, and declared regression inputs;
- `FreezeRecord`: source tag, Active version, library/API hashes, Git clean, old Active immutability, and adapter owners.
- `PlaygroundCleanupRecord`: experiment disposition, archived evidence path, removed Playground paths, cleanup actor, and timestamp.

## Cross-record graph rules

The records are one graph, not independent JSON files:

```text
GoalClarificationRecord (confirmed/admitted)
  -> EvidenceRecord
  -> ReviewRecord
  -> PromotionRecord
  -> RegressionReport
  -> FreezeRecord
```

No implementation claim, Playground mutation, formal red test, promotion, or issue closeout is admitted while the goal record is `received`, `parsed`, or `clarification_pending`.

Required checks:

- `ReviewRecord.promotion_id` resolves to `PromotionRecord.promotion_id`;
- every `ReviewRecord.evidence_ids` resolves to an EvidenceRecord;
- `ReviewRecord.reviewed_commit == PromotionRecord.source_commit`;
- `PromotionRecord.artifact_hash == FreezeRecord.library_hash`;
- `PromotionRecord.new_active_version == FreezeRecord.active_version`;
- `FreezeRecord.promotion_id` resolves to the promoted record;
- `FreezeRecord.promotion_record_hash` matches the referenced PromotionRecord;
- `FreezeRecord.artifact_record_id` resolves to the published artifact evidence;
- `FreezeRecord.regression_report_id/hash` resolves to the exact passing RegressionReport;
- review verdict is `pass` for the referenced commit, scope, and artifact.

These checks belong to a record-reference gate. Individual schema validity is insufficient.

## Evidence freshness

Evidence includes `expires_at`, `input_hashes`, `source_commit`, `artifact_hash`, and `scope_hash`.

Evidence is invalid when:

- source commit changes;
- scope hash changes;
- input/artifact hash changes;
- expiry is reached;
- reviewed commit changes after review.

AI confidence is evidence metadata and review input. It is not a promotion result. `review_verdict=pass` is the promotion admission result.

## Regression freeze gate

Unit and focused tests may be whitebox-only. Regression suites and bug reproduction must include both whitebox and blackbox evidence. Freeze requires a non-zero passing report with no disallowed skips, exact command/suite identity, and matching source, scope, artifact, public API, and input hashes.

After freeze, ordinary execution of the unchanged module's full regression suite may be disabled. The suite declaration and report remain immutable verification inputs. Source, contract, public API, artifact, or dependency changes invalidate the report and require regression re-enablement before a new version can freeze.

## Separated lifecycles

```text
Issue:          open -> playground -> review -> promoted -> closed
Library:        draft -> compiled -> verified -> active -> retired
Source snapshot:mutable -> merged -> protected
Artifact:       generated -> verified -> published -> immutable
```

No state in one lifecycle implies a state in another. A closed issue does not imply an Active library; an Active library does not imply a Protected source; a Protected source does not imply a verified artifact.

## Version relation

```text
Active v1
  -> change request
  -> Playground based on v1
  -> review PASS
  -> Active v2
  -> v1 immutable history
```

Every new version records `previous_active_version`, `base_source_commit`, `base_library_hash`, `change_set_id`, `migration_id`, and `compatibility_level`.

`appsdk begin-version` creates the machine binding before source changes: `previous_active_version`, `new_active_version`, `base_artifact_hash`, and `base_source_commit`. Promotion and Freeze records for the new version must match that binding; publishing clears the mutable binding only after the new Active version is committed.

Every formal debug merge also records `root_cause`, `design_id`, and `change_reason_comment`; promotion is rejected without them. Every closed experiment records its cleanup disposition so Playground cannot grow without bound.
