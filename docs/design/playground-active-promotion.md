# Playground → Active Promotion Contract

## Purpose

AppSDK separates mutable source, the current consumable library, frozen history, and compiler output.

```text
Issue
  -> Playground experiment
  -> red/green evidence
  -> architecture review
  -> review PASS
  -> merge source to mainline
  -> compile library
  -> Active library
  -> move source/contracts to Protected
  -> git/artifact lock
  -> issue close
```

## Zones

### Playground

Playground owns experiments only:

- debug instrumentation;
- failing samples and red tests;
- candidate feature/module implementations;
- candidate contracts;
- design evidence.

Playground is never a runtime input, formal owner, release input, or fallback path.

Every experiment carries `experiment_id`, `base_commit`, `base_artifact_hash`, hypothesis, positive/negative signals, and evidence paths.

Every experiment also has a cleanup disposition. After review it must be archived under Protected history with a cleanup record or physically removed from Playground. `archive_then_remove` is the default; an open experiment needs an owner, issue, expiry, and next action. Closed work may not leave unbounded experiment directories.

### Active library

Active is the immutable, current consumable library surface. It is not active source code. Runtime and downstream modules consume `active/lib/**`; they never consume Playground source.

An Active library version enters through a promotion record after architecture review and compilation. Direct copy, direct patch, and dual ownership are not promotion mechanisms. The old Active library remains immutable as a historical version; it cannot become fallback or shadow writer.

Changing a frozen module starts with an explicit version opening:

```bash
appsdk begin-version <project> --module <id> --from <current-version> --to <new-version>
```

The command verifies the current Active index, frozen record, previous artifact hash, and Protected history; moves the old Protected archive and record graph into versioned history; records `version_base`; and reopens only that module at `source_implemented`. It never edits or deletes the previous Active library.

### Protected source and contracts

Protected contains source and contracts after a successful promotion. It is the historical source of the Active library. Git must record the source commit/tag, artifact hash, public API hash, review PASS, and previous Active version. Protected + Git support audit, detection, and recovery; they do not prevent a same-repository shell agent from reading the source.

### Generated

Generated contains compiler outputs and indexes only. It is never hand-edited. The current Active library is a published/consumable output and must record its source commit/tag, library hash, public API hash, and review verdict.

## Promotion requirements

A promotion proposal must bind:

- `issue_id` and `experiment_id`;
- `feature_id` and target `module_id`;
- old/new owner;
- resource bindings and mainline edges;
- allowed/forbidden paths;
- required tests and gates;
- retirement plan for the old owner;
- target artifact change.

Promotion requires:

1. a red test or failing sample;
2. positive and negative validation;
3. confirmed first divergence and unique owner;
4. no fallback, dual write, or payload/control leakage;
5. architecture review with explicit `PASS`;
6. a clean mainline source merge followed by formal verification;
7. compilation of a new library version;
8. lock evidence: Git clean, source commit/tag, library hash, public API hash, review PASS, and old Active library immutable.
9. for debug changes, a merge comment records the root cause, approved design ID, unique owner, and why the formal change is the smallest architecture-compliant fix;
10. a Playground cleanup record identifies archived evidence and removed paths.
11. a RegressionReport proves both whitebox and blackbox regression coverage for the exact freeze candidate.

## Lock and closeout

Issue closure and architecture freeze are separate state machines. A feature can close at `architecture_stable` while internal implementation remains mutable. Contract, resource identity, owner, mainline edge, or artifact changes after freeze require a new version or migration.

Closeout records the source commit/tag, Active library version/hash, public API hash, review verdict, required gates, runtime evidence when applicable, protected historical source, previous Active version, and remaining risks.

Frozen unchanged modules may disable ordinary full-regression execution, but never delete the suite or report. A source, contract, API, artifact, or dependency change invalidates the report and reopens the regression gate.
