# Contracts and Failures

## Severity

- `advisory`: recommendation; never blocks.
- `warning`: visible debt; current work may continue.
- `forbidden`: unsafe or false transition; command stops.

Forbidden defaults:

- fabricated/empty evidence for required PASS;
- non-adjacent transition within a selected workflow;
- plan/event history overwrite or deletion;
- stale project, goal, source, tree, scope, owner, Skill, AGENTS, or manifest;
- committed governance mutation from main;
- false review, delivery, promotion, freeze, or cleanup completion.

Legacy optional metadata, historical strictness outside changed scope, missing
release evidence during ordinary development, and absent parallel-worker data
in a single-worker task are warnings or advisory.

Architecture conformance is change-scoped. New or modified behavior that puts
control truth in payload/metadata/log context, duplicates an owner or
implementation in a way that breaks its contract, or mocks a required capability
is forbidden. A project-declared operation/hook/gate boundary remains binding.
Optional simplifications are advisory; direct code is not a violation merely
because it could use configuration. The same pattern in untouched historical code
is advisory unless it affects safety, ownership, evidence truth, or the current
delivery boundary.

## Compatibility

Project-scoped commands resolve the project from the process `cwd` when their
optional project argument is omitted. Do not require a project-root environment
variable. `--help` is project-independent and must never resolve or validate a
project.

Harness does not check binary SHA. Existing AppSDK version/contract compatibility
belongs to canonical lifecycle commands. A mismatch must return either a
supported migration route or an authorized reset/reinitialize route; it must
not trap development behind repeated byte-identity checks.

`GUIDANCE_SETUP_REQUIRED` is not a lifecycle failure. It means existing
governance has no approved Guide declaration. Only when Guidance is selected, run the returned read-only
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

Every claim binds one branch and owner worktree. Resource close requires remote
receipt when delivery is in scope, retention/cleanup record, worktree removal,
removal verification, then claim release. Engineering delivery may be complete
while an owned worktree is retained and its cleanup obligation stays open. An abandoned or foreign dirty
worktree is preserved until its owner or explicit cleanup authorization exists.
