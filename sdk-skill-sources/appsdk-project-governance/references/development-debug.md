# Development and Debug

## Feature or new project

When persistent Guidance is selected, project the task-specific intake:

```bash
appsdk guide init --task <task-id> --mode develop --module <module-id>
```

Read the returned AGENTS and Skill sources, invoke the suggested Skills, and ask
only unresolved questions. A PlanProposal is required only for the selected
Guidance workflow. Otherwise establish the same goal/scope directly and proceed.

Before implementation, bind the project context:

```text
requirements + acceptance + non-goals
-> declared function owner + allowed/forbidden paths
-> declared verification gates + prior records
-> proportionate architecture and detailed design
```

Architecture and detailed design are required for a new project or meaningful
cross-module or semantic change. A local change may use the existing design and
use it directly. Within a selected Guidance workflow, use its declared bypass.

Then:

```text
latest origin/main
-> clean owner worktree
-> minimal implementation
-> candidate commit
-> verification/review/delivery
-> latest-main integration
-> remote receipt
-> lifecycle close
-> worktree/claim cleanup
```

Candidate commit binds tree/artifact/evidence. It is not a delivery commit and
does not authorize merge.

Before adding behavior, perform an ablation check: confirm that the behavior is
necessary, no declared owner already provides it, and common semantics can
reuse an existing shared function. Preserve a fixed lifecycle skeleton only
when the project declares one. Choose direct code or configuration by actual
complexity, not by a universal preference. Missing capability fails or skips with a
reason. Control/configuration truth belongs only in declared typed control
resources, error chains, or project configuration sources, never business
payloads, metadata, debug logs, or implicit context.

## Debug

When persistent Guidance is selected, start with:

```bash
appsdk guide init --task <task-id> --mode debug --module <module-id>
```

Use the projected questions to bind the real failing sample and experiment
contract before writing the debug PlanProposal.

Use one hypothesis per round:

```text
read AGENTS + declared maps + prior notes/records
-> append observations, one hypothesis, and evidence to run notes
-> same-input reproduction when feasible
-> confirmation/falsification signals + first semantic divergence
-> forward and reversal intervention when feasible
-> unique-owner fix + proportionate regression
-> mapped gates + old-input replay when runtime-impacting
```

Final error is not automatically root cause. Grep hit is not evidence. Do not
patch output layers, add fallback, or modify multiple owners to make one test
green.

Before merge, review maps, module boundary, owner, payload/control separation,
configured operations, registered hooks, declared gates, ablation, shared
function reuse, affected tests, and the latest-main integrated tree. A violation
introduced or modified by the candidate blocks review. An untouched historical
violation is a recommendation unless it affects the changed scope, safety,
ownership, evidence truth, or required delivery.
