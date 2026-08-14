---
name: appsdk-project-governance
description: Integrate and operate AppSDK governance in a new project without copying the SDK implementation into the project. Use for project bootstrap, goal clarification, Playground changes, evidence/review/promotion, Active publishing, Protected freezing, SDK lock verification, and boundary audits.
---

# AppSDK Project Governance

Use the external AppSDK implementation as the governance engine. Keep only project contracts and lifecycle records in the target repository.

## Boundaries

- External AppSDK: compiler, CLI, schemas, harness, adapters, immutable rules.
- `.appsdk/`: committed project governance contract, maps, goal, records, verification, and `sdk.lock`.
- `.appsdk-control/`: ignored local run state, review cache, temporary harness output, and worker state.
- `playground/`: mutable experiment source.
- `active/lib/`: immutable consumable library.
- `protected/`: frozen source, contracts, and history.
- `generated/` or the project-declared artifact root: compiler output only; never hand-edit.

Never copy the AppSDK source, compiler, or harness into the business project. Never put committed project maps or lifecycle records in `.appsdk-control/`.

## Existing project bootstrap

For an existing project, begin with a preparation record instead of manually creating governance directories:

```bash
appsdk prepare <workspace>
```

The AI must read `.appsdk-prepare.json`, ask the user about change kind, project root, legacy roots, new roots, Protected roots, allowed paths, forbidden paths, and payload/control separation, then update the record to `status: confirmed`. Do not initialize before confirmation.

```bash
appsdk init <workspace> --project-root <relative-path>
```

`init` is idempotent. The workspace may contain legacy code; the configured relative path becomes the new AppSDK project root. It creates the four governance zones and `.appsdk-control/` under that root, fills only missing governance files, and appends one managed `.gitignore` block for local control state, compiled Active libraries, and generated outputs. It preserves existing project files and existing ignore rules. Absolute paths and `..` traversal are rejected. Use `appsdk new` only for an empty destination.

## New project flow

1. Install or reference a pinned external AppSDK.
2. Create the project from a template into an empty, non-symlinked destination.
3. Confirm `.appsdk/project.json`, `.appsdk/goal.json`, module ownership, zone roots, and `sdk.lock`.
4. Run `appsdk verify` before source changes.
5. Clarify the user goal: restate objective, acceptance criteria, non-goals, assumptions, ambiguities, and questions.
6. Do not claim, edit Playground, create a formal red test, compile, or promote while goal status is `received`, `parsed`, or `clarification_pending`.
7. After user confirmation, bind scope, owner, allowed/forbidden paths, and required gates.
8. Work only in the module's Playground scope. Produce EvidenceRecords, then ReviewRecord with AI confidence and reviewed artifact/scope hashes.
9. Promote only when the record graph is valid and evidence targets the exact module and candidate artifact.
10. Before freeze, run the module's declared regression suite and create a RegressionReport bound to the exact source commit, artifact hash, public API hash, scope hash, and input hash. Regression and bug-reproduction evidence must combine whitebox and blackbox coverage; unit and focused tests may be whitebox only.
11. Compile the candidate library, publish the immutable Active artifact, archive source/contracts to Protected, create FreezeRecord with the RegressionReport ID/hash, and verify.
12. After freeze, ordinary full-regression execution for that unchanged module may be disabled to reduce CI load. Keep the suite and report. Any source, contract, public API, artifact, or dependency input change invalidates the report and requires regression re-enablement before a new freeze.
13. Close every experiment with a PlaygroundCleanupRecord; archive evidence to Protected history, then remove the experiment directory under the declared retention policy.

For a frozen module change, run `appsdk begin-version <project> --module <id> --from <current> --to <new>` before formal source edits. The command must bind and preserve the current Active artifact, Protected archive, and record graph; direct edits to the old Active or Protected version are forbidden.

## Debug flow

Clarify goal first. Then use evidence-first debugging: baseline, first divergence, positive intervention, negative intervention, and unique owner. Experiments stay in Playground. Formal fixes go through the same review, promotion, compile, and freeze gates, with a merge comment recording root cause, approved design ID, owner, and reason.

## Required checks

- `appsdk verify <project>`
- project tests and required gates
- candidate artifact hash and public API hash
- record graph references, freshness, scope, module, and version relations
- RegressionReport whitebox + blackbox coverage, non-zero passing tests, exact input binding, and FreezeRecord report hash
- Protected and Active immutability
- final architecture review with explicit PASS

Do not claim lifecycle completion from unit tests alone. A missing external adapter, review verdict, installation, restart, or online evidence is an explicit remaining gap.

Read `docs/design/appsdk-project-integration.md` for the repository layout and `docs/design/playground-active-promotion.md` for promotion semantics.

Read `references/goal-prompt.md` when Jason gives a new goal and asks you to turn it into an executable `/goal` prompt.
