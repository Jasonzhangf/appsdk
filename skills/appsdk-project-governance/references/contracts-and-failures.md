# Contracts and Failures

## Severity

- `advisory`: recommendation; never blocks.
- `warning`: visible debt; current work may continue.
- `forbidden`: unsafe or false transition; command stops.

Forbidden defaults:

- fabricated/empty evidence for required PASS;
- non-adjacent transition;
- plan/event history overwrite or deletion;
- stale project, goal, source, tree, scope, owner, Skill, AGENTS, or manifest;
- committed governance mutation from main;
- false review, delivery, promotion, freeze, or cleanup completion.

Legacy optional metadata, historical strictness outside changed scope, missing
release evidence during ordinary development, and absent parallel-worker data
in a single-worker task are warnings or advisory.

## Compatibility

Harness does not check binary SHA. Existing AppSDK version/contract compatibility
belongs to canonical lifecycle commands. A mismatch must return either a
supported migration route or an authorized reset/reinitialize route; it must
not trap development behind repeated byte-identity checks.

`GUIDANCE_SETUP_REQUIRED` is not a lifecycle failure. It means existing
governance has no approved Guide declaration. Run the returned read-only
bootstrap intake, present `GuidanceSetupProposal`, obtain explicit user
approval, then update and compile project-owned rule sources. Do not report an
external AppSDK blocker or retry compile against an undeclared rule set.

## Failure output

Required fields:

```text
first failing gate/code
project/module/lifecycle projection
preserved state
retry_allowed
canonical owner
one executable next action
```

Do not retry unchanged failures. Do not hand-write records or hashes. Fix the
first divergent owner, revise the plan if bound context changed, then execute
once.

## Worktree audit

Every claim binds one branch and owner worktree. Completion requires remote
receipt when delivery is in scope, retention/cleanup record, worktree removal,
removal verification, then claim release. An abandoned or foreign dirty
worktree is preserved until its owner or explicit cleanup authorization exists.
