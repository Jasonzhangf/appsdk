---
name: appsdk-project-governance
description: Apply AppSDK engineering quality gates, project contracts and lifecycle evidence. Use optional Guidance for planning; keep automatic multi-worker Collab separate from quality admission.
---

# AppSDK Project Governance

## Purpose and mandatory boundary

AppSDK verifies engineering quality. Collab supports automatic multi-worker
registration, communication and task/file ownership. Memory and Guidance help
when useful. Missing auxiliary state does not fail independent development.

Default flow: understand goal/scope → implement → relevant verification →
review → authorized delivery. Require applicable quality, safety and evidence
integrity gates; do not turn every available command into a mandatory phase.

Run project commands from project cwd. An explicit optional project path is for
operators intentionally working elsewhere; no project-root environment variable.

## One global AppSDK binary

Do not copy or select AppSDK binaries by hand. The AppSDK repository's only
supported global installation entry is:

```bash
scripts/install-global-appsdk.sh
```

It builds the release, atomically replaces the executable beside the active
`cargo`, removes exact AppSDK-managed legacy copies, and checks that one
managed `appsdk` remains. Run it from any directory; it resolves its own
repository root. SHA-256 is diagnostic output only, not a fixed admission
condition. Do not stop project development because a historical binary hash
differs. If the version or command path is wrong, run the installer once and
refresh the current shell cache (`rehash` in zsh or `hash -r` in bash); do not
manually copy, rename, or leave `.local/lib/appsdk/<version>/appsdk` beside the
canonical entry.

An AppSDK binary install does not restart a daemon. Use the daemon's official
maintenance command separately when the running process must load the new
binary. Never start v2 or create a second global AppSDK entry as a workaround.

## Reset legacy governance

When an old version has left incompatible records, stale audit reports, or
rebuildable delivery output, do not patch or hand-edit those files. In a clean
non-`main` owner worktree, after the user explicitly authorizes discarding the
named legacy control plane, run once:

```bash
appsdk reset-governance --discard-legacy
appsdk init
appsdk guide init --task governance-reset --mode bootstrap --module app-core
appsdk guide compile
appsdk verify
appsdk compile
```

`reset-governance` is idempotent. It removes the old `.appsdk/` records and
maps (including old audit/migration reports), `.appsdk-control/`, and the
rebuildable `generated/` projection, then creates a fresh current contract and
writes a reset record. It preserves business source, runtime data, `active/`,
and `protected/` by default. Existing Active/Protected artifacts are not
silently deleted; if they are obsolete, request exact paths and perform a
separate authorized cleanup with its own evidence. Never run reset in `main`,
on a dirty worktree, against another worker's claim, or while inventing a
migration record. A reset is a new governance baseline, not proof that old
delivery or review was completed.

### Legacy audit and delivery output handling

Before reset, classify every old item; do not treat a filename as proof of
current truth:

- `.appsdk/records/**`, `.appsdk/transactions/**`, and AppSDK audit/migration
  reports are legacy control-plane state. Preserve a small inventory or
  immutable snapshot in the run note when audit history matters; then let
  `reset-governance --discard-legacy` remove the old control plane. Do not
  copy old PASS, hashes, receipts, or review results into the new baseline.
- The project-declared `governance.generated_root` and module-declared
  rebuildable outputs are disposable delivery projections. The reset command
  removes the declared generated root (plus the standard `generated/` root)
  without deleting business source or published state.
- Failed transaction staging under `.appsdk/` is removed with the discarded
  control plane. If it belongs to a still-valid current task, use that task's
  canonical retry/abort operation before reset; never delete staging by hand.
- Reports or outputs outside those declared roots (`dist/`, `.deploy/`,
  `build/`, `tmp/`, custom report folders, or vendor output) are not assumed
  disposable. Retain them or archive them until the project owner identifies
  the exact path as rebuildable and authorizes its separate cleanup.
- `active/`, `protected/`, runtime data, source, and human project documents
  are retained by reset. Removing obsolete Active/Protected or historical
  documents requires exact paths, explicit authorization, and a separate
  cleanup record.

After reset, report removed and retained classes separately. A clean directory
is not evidence that delivery, review, install, restart, or freeze happened.

## Working loop

1. Read project AGENTS and affected code/contracts. Resolve owner, scope,
   acceptance and relevant gates. Read historical notes only when they help.
2. Use a clean owner worktree from latest origin/main. Preserve others' work.
   For multi-worker work, automatically register the peer, communications and
   task/file scope through official Collab; enforce overlap/resource ownership.
3. Implement the smallest adequate change. Use existing design for local work;
   clarify only material unknowns. Do not require a new plan or approval when
   scope is already authorized and clear.
4. Run applicable tests, necessary build and actual entrypoint checks. Install
   and restart only when required by the delivery object. Fix failures at their
   owner, never forge evidence or hide errors.
5. Review exact validated changes under the shared review standard. Block
   concrete correctness, safety, contract or material structural regressions;
   optional simplifications are advisory.
6. Reuse still-valid evidence when relevant source, inputs, dependencies,
   configuration, artifact and environment remain unchanged. Rerun affected
   checks after changes; verify an altered integration candidate.
7. Deliver within authorization. Report test, review, merge, install, publish
   and resource cleanup as separate achieved states.

## Optional Guidance

Use `appsdk guide status/init/plan/update/next/close` when the user/project
selects persistent planning or a long task benefits from recovery. Default
`advisory` and `warning` do not require a task plan or setup before development.
Missing PlanRecord does not fail ordinary `verify` or `compile`.

When using Guidance, follow its declared transitions and bind observations to
the current context. A failed optional workflow is not a failed quality gate.
Do not fabricate a successful step to close a plan.

For a requested setup/upgrade, `guide init --mode bootstrap` is read-only.
Compare current project-owned sources with the advisory standard template;
apply only authorized rule changes. `appsdk init` refreshes SDK resources but
never overwrites project AGENTS, Skills, records, Active or Protected. Merely
auditing rules does not require running initialization or changing setup.

## Automatic Collab

Persistent subagents and project-specific notification policy:
[subagents-config.md](references/subagents-config.md). All policies live in
`~/.appsdk/config.toml`; `appsdk config` shows effective configuration.
`appsdk subagent start/list/status/send/close` delegates to the Collab owner.

`appsdk init` attempts official `collab init` once in a live tmux peer, preserving
the inherited environment. Successful initialization registers identity and the
finite direct-message subscription. Do not duplicate that initialization.
Once task/worktree scope is known, use the Collab task lifecycle to register
feature/resource and file ownership before concurrent edits. Do not invent
task scope inside AppSDK initialization.

No tmux peer means pending. An unavailable/failed Collab reports its error;
independent work continues, while operations requiring shared ownership wait.
Keep automatic communication and file/task collaboration enabled; a serial
merge queue is required only when the project selects that integration mode.
Its ownership and tested-integration protections remain mandatory.

## Evidence and state ownership

- Project AGENTS owns project facts; Skills own procedure; declared machine
  contracts own enforceable gates. Existing lifecycle records remain the sole
  evidence truth. Plans, notes and Collab statuses do not duplicate PASS.
- Runtime review admission retains whitebox, public-entrypoint blackbox and
  exact candidate/artifact/environment identity. Module `deployment_operations`
  declares required `install`/`restart` receipts; omission retains both for
  compatibility, `[]` means neither operation applies. Bind this choice before
  validation; changes invalidate artifact identity. Every supplied receipt is
  checked. A missing required capability remains a blocker.
- Review confidence scores are optional annotation, never proof of quality.
- Freeze/Active/Protected apply when immutable artifact publication is in
  scope. Do not require freezing for a documentation edit or ordinary review.
- Engineering delivery may complete with a retained worktree. Keep ownership
  and cleanup obligations explicit; only claim resource closure after actual
  safe cleanup. No forced deletion to make a task appear complete.
- Memory is optional. No automatic durable memory/rule promotion. Long tasks
  and handoffs may record concise decisions and references to existing evidence.
  Memory migration and re-entry are explicit independent operations: use
  `project-memory migrate` for a source-preserving, resumable schema move and
  `project-memory index|export` to render old and current raw records as a
  Markdown index/details directory; after an intentional detail edit, use
  `project-memory import` to append the change back to raw history. Markdown
  is an interchange view, not a second truth store.
  Normal memory writes use one `project-memory entry` invocation, which writes
  the raw event and regenerates detail/index/projection together; do not hand
  write one of those derived files as a separate step.
  `memory/index.md` contains fixed-size Skill description candidates. Their L2/L3
  lines already include the kind, tags, and relative `L2/` or `L3/` detail path.
  During initialization or an intentional refresh, manually carry deduplicated
  L1 lines into the project Skill description, then fill unused slots with L2
  and L3 lines. Memory writes never rewrite Skill descriptions automatically.
  `project-memory reentry [project] --run <run-id>` to resume the same run after
  interruption. A missing or rebuilding memory index is not a governance
  failure, and memory state must not be reconstructed from Guide, debug,
  develop, or log payloads.

## References: load only the relevant domain

- Initialization or migration: [bootstrap-migration.md](references/bootstrap-migration.md).
- Development/debug: [development-debug.md](references/development-debug.md).
- Runtime review/delivery/freeze: [review-delivery.md](references/review-delivery.md).
- Selected persistent planning: [process-control-harness.md](references/process-control-harness.md).
- Contract errors/compatibility: [contracts-and-failures.md](references/contracts-and-failures.md).
- Explicit goal-prompt request: [goal-prompt.md](references/goal-prompt.md).

Failure reports name the failed applicable gate, preserved state, owner and next
action. Never infer deployed success, merge, freeze or cleanup from an earlier
test or an auxiliary workflow close.
