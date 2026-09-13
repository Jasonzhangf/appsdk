# Governance gate audit and ablation

## Scope

This review covers the AppSDK CLI gate path and the bundled AppSDK governance
Skill. It does not change Collab runtime, RouteCodex, consumer project records,
or the wire role tokens used by existing Collab installations.

The acceptance rule is proportionality: a gate is mandatory only when it
protects quality, safety, ownership, evidence truth, or the delivery surface
selected by the project. Coordination, Guidance, Memory, long-horizon prompts,
installation, restart, freeze, and live replay remain conditional capabilities.

## Required gates retained

| Gate | When it applies | Why it remains |
| --- | --- | --- |
| Contract, schema, path and hash checks | Every applicable `verify` or mutation | Prevents tampered control truth and unsafe paths. |
| Confirmed goal | Mutations, compile, promotion, review admission and lifecycle record production | Binds an intentional change; ordinary read-only `verify` may inspect an unconfirmed project. |
| SDK lock/resource integrity | Commands that consume the current SDK bundle | Prevents compiling or publishing against an unknown bundle. |
| Stage transition and artifact identity | Compile, promotion, freeze, publication and admission | Keeps lifecycle transitions adjacent and binds output to source/tree/artifact. |
| Regression contract | Before freeze/retirement, with declared re-entry on relevant drift | Protects immutable releases without forcing unfinished modules to run release regression. |
| Review/admission evidence | Runtime delivery or immutable publication selected | Confirms candidate, artifact, environment, producer and public entrypoint. |
| Historical migration integrity | Admission and explicit migration/reset flows | Prevents old records from masquerading as current evidence. |

## Removed or demoted duplication

1. `compile` validates the project once and reuses that project value while
   compiling each module. Because each module build is an external command, a
   small control-input snapshot still checks root/worktree, goal, project,
   scenario and declared-contract identity at every module boundary. The
   standalone `compile-module` entry keeps its own full checks. No safety check
   was removed from an independent command or crossed build boundary.
2. The top-level governance Skill no longer repeats the complete delivery
   ceremony already owned by `references/review-delivery.md`. It now states the
   applicability rule and points to the identity-bound procedure. A later
   phase does not rerun an unchanged external test, merge, deployment or
   publication action.
3. The Skill no longer embeds a second SDK-source identity state machine. Collab
   owns live identity and route proof; AppSDK only waits when the selected
   operation actually needs shared ownership or communication. Missing Collab
   state therefore does not block independent AppSDK development.
4. User-facing role and prompt text uses **subworker**. The new
   `appsdk subworker` entry forwards to the existing Collab compatibility token
   without creating a second registry or native Desktop task/thread. Existing
   wire/config tokens remain unchanged for compatibility.

## Re-entry rule

Each lifecycle phase persists candidate/tree, scope, dependency, artifact,
producer, environment and freshness identity with its PASS evidence. An
unchanged, valid projection is reused after lightweight integrity checks. A
drift invalidates only that phase and downstream dependants; a session or
phase-name change alone never starts a full rerun. `verify` walks the graph but
does not execute external tests, deployment, merge or publication actions.

## Non-goals

- No change to Collab daemon, mailbox, tmux or native app-server transport.
- No migration, reset or deletion of consumer project history.
- No weakening of admission, path safety, immutable Active/Protected history,
  or evidence authenticity.
- No forced global install or daemon restart for this source-only change.
