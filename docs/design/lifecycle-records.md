# Lifecycle Record Contract

String requirements are only declarations. A closed lifecycle requires records with evidence, producer, scope, identity, and freshness.

The lifecycle producer treats `bug_triage.query_executed: true` as a typed caller
attestation. It independently validates the supported mode and exact issue-ID
tokens, rejects mismatched or duplicate identities, and records
`bug_triage_query_binding` as the SHA-256 binding of issue ID, query, mode, and
reopen source. Empty, `none`, and `legacy-*` issue IDs remain exempt.

The confirmed goal issue is the parent scope for a change. The
`WorktreeRecord.issue_id` is the module execution scope and may be a narrower
module issue. When those IDs differ, the producer requires
`WorktreeRecord.goal_issue_id` to equal the confirmed goal's issue ID exactly,
then computes `goal_issue_binding` itself from the goal and module issue IDs.
The binding is the SHA-256 of the canonical object
`{"goal_issue_id": <goal issue>, "issue_id": <module issue>}`, is persisted in
the WorktreeRecord, and is revalidated on reuse. A missing or forged
`goal_issue_id` fails closed. Reproduction and baseline evidence remain bound
to the module issue and its triage evidence.

## Module registry binding

Each `project.json#/modules[]` entry has an exact registry binding by default.
The producer may use only the registry entry whose `module_id` equals the
project module ID, and that entry must be active and owned by the project's
`source_owner`. A project module that intentionally spans finer-grained
registry modules must declare the boundary explicitly:

```json
{
  "module_id": "routecodex-v4-runtime",
  "source_owner": "routecodex-v4-runtime",
  "registry_binding": {
    "mode": "aggregate",
    "modules": [
      "routecodex-v4-runtime",
      "routecodex-v4-runtime-bin",
      "routecodex-v4-provider",
      "routecodex-v4-router"
    ]
  }
}
```

`mode: "aggregate"` requires a non-empty, unique module-ID whitelist. Every
listed entry must exist exactly once, be active, have a non-empty owner and
valid owned paths, and cover every project `owned_paths` declaration. When a
same-ID registry entry exists, aggregate mode must list it as well; its active
status and owner binding remain mandatory. Registry entries outside the
whitelist never participate in path coverage. The producer validates this
contract before writing any lifecycle record, and the normalized binding is
part of the producer scope hash so changing the boundary invalidates record
reuse.

## Records

- `GoalClarificationRecord`: raw request, restated objective, acceptance criteria, non-goals, assumptions, ambiguities, questions, scope, confirmation, and admission status;
- `EvidenceRecord`: one red/positive/negative test, replay, build, artifact, runtime, or gate result;
- `ReviewRecord`: reviewer identity, reviewed commit, verdict and evidence IDs;
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

## Stage execution, re-entry, and reuse

The lifecycle chain is evaluated one persisted phase at a time. The phase
projection is bound to the candidate and tree, module scope, dependency
records, artifact and environment, map hashes, evidence IDs, and any
mainline/cleanup identity required by that phase.

Each invocation performs a read-only integrity, identity, and evidence-
freshness check for the current phase and its upstream records. A PASS
projection with the same complete identity and unexpired evidence is returned
with `reused: true`; the external action that created that evidence is skipped.
This is phase-local reuse. It does not claim that tests, deployment, merge, or
publication ran again. `verify` may therefore reread the complete graph to
detect tampering or drift without rerunning external commands.

When a candidate, dependency, map, artifact, environment, input, or evidence
expires, that phase and its downstream phases lose reuse eligibility. The old
PASS record remains immutable and is not rewritten or downgraded; the next
attempt must produce a new candidate-bound projection. A non-PASS projection
(`fail`, `unknown`, or any invalid status) is never a cache hit. Repeating the
same non-PASS identity returns `LIFECYCLE_CHAIN_STAGE_NOT_PASS`; a changed
identity archives the previous projection and may re-enter the phase. The
archive is append-only JSONL at
`.appsdk/records/attempts/<module>/<phase>.jsonl`, with a record hash and the
full prior projection. Invalid, conflicting, or edited attempt entries fail
closed.

The WorktreeRecord, ReproductionRecord, and baseline EvidenceRecord producer
uses the same phase-local rule. It skips the baseline command only when the
complete three-record set, declaration, command, identity, output hash, and
evidence freshness all match. Missing members, partial sets, or drift are
explicit errors and are never filled from a guessed cache.

The current projection is separate from its historical witness. When a new
candidate or baseline identity is observed, the producer executes the declared
baseline command, appends the previous Worktree/Reproduction/baseline set to
`.appsdk/records/attempts/<module>/producer-records.jsonl` with its original
bytes and hashes, then atomically replaces only the current projection. The
same input identity is reused on later calls. A partial current set, malformed
archive, conflicting hash, or interrupted transaction remains fail-closed; the
producer never deletes, edits, or relabels the old witness.

Every producer archive is revalidated before reuse or append. The validator
recomputes the record-set hash and stable archive ID, checks the timestamp and
JSONL framing, and binds the three historical paths to the module's
Worktree/Reproduction/baseline records. A durable producer transaction is
recovered from its marker before a new baseline is executed or the current set
is archived; this prevents a partially published replacement from being
misread as a new incomplete current set.

For a project nested inside a registered Git worktree, the baseline checkout
preserves the project's path relative to that Git worktree. The declared
working directory is then resolved from the nested project root, so `.` and
other relative command paths observe the same project scope in the baseline
that they observe in the candidate.

Downstream `review`, `effectiveness`, `merge`, and `promotion` projections use
the same rule. An exact PASS identity is reusable. A changed identity archives
the old PASS as `result: "stale"` in the phase attempt ledger and writes a new
candidate-bound projection after all upstream gates pass. PASS history remains
immutable evidence and is never treated as current evidence for the new
candidate.

Frozen dependency artifacts are restored only through the explicit
`appsdk rehydrate-frozen --module <id>` operation. `verify` and `compile` do not
copy protected/history files, manufacture an Active index, or implicitly publish
a frozen source. After rehydration, the affected compile/verify phase and its
downstream dependants are rerun against the restored artifact identity.

## Evidence freshness

Evidence includes `expires_at`, `input_hashes`, `source_commit`, `artifact_hash`, and `scope_hash`.

Evidence is invalid when:

- source commit changes;
- scope hash changes;
- input/artifact hash changes;
- expiry is reached;
- reviewed commit changes after review.

AI confidence is optional annotation, never a required admission field or proof. `review_verdict=pass` and its validated evidence are the review admission result.

## Regression freeze gate

Unit and focused tests may be whitebox-only. Regression suites and bug reproduction must include both whitebox and blackbox evidence. Freeze requires a non-zero passing report with no disallowed skips, exact command/suite identity, and matching source, scope, artifact, public API, and input hashes.

The module `regression` declaration is required only when a module is `frozen`
or `retired`. Development stages may omit it so a local change can use focused
checks without manufacturing a freeze report. A declaration that is present in
an earlier stage is still validated; `required_before_freeze=false` is invalid
once a module reaches `architecture_stable`, and omission is the only permitted
shortcut. `verify` and its read-only `--admission` preflight accept a missing
goal record; mutation, promotion, review admission, and lifecycle producers
still require a confirmed goal.

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
