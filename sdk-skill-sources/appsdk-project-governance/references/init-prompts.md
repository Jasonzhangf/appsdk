# AppSDK initialization and optional Collab coordination

Use [bootstrap-migration](bootstrap-migration.md) for AppSDK project admission.
Enable Collab only when explicitly requested; independent work needs no registration.

## State ownership

Global truth:

```text
~/.appsdk/projects.jsonl
~/.appsdk/runtimes.jsonl
~/.appsdk/communication.jsonl
~/.collab/server.sock
~/.collab/events.jsonl
~/.collab/log.txt
```

Project-local `.appsdk/`, `.appsdk-control/`, and `.agent-collab/` are not the
global truth. `.appsdk/` is the committed project contract/maps/records;
`.appsdk-control/` is ignored local run/cache state; `.agent-collab/` is a
project discovery marker, not the current reducer store. Handle them only through
the AppSDK reset or Collab migration/reset owner. Do not inspect or edit them
to decide whether the peer is registered.

## Registered collaboration

For requested registration or recovery, use the current project directory:

```sh
collab context
```

Use the returned current identity and role facts. Supply only missing current
host facts through the [recovery contract](../../collab/references/stable-name-recovery.md).
Do not repeat context when the current result already answers the question.
Preserve daemon/runtime errors and stop the dependent identity action; continue
independent authorized work. Global daemon lifecycle remains user-managed.

Ordinary AppSDK project initialization runs only in the canonical project main
checkout. Agent identity bootstrap is `collab context`; it may run from the
project or worktree, and the daemon resolves the canonical route. A Git
worktree contains tracked `.appsdk/` files but does not inherit ignored
`.agent-collab/` or `.appsdk-control/` state. The separate authorized AppSDK
`--fresh --discard-legacy` reset may run from its clean non-main owner worktree
as specified below. Inside a worktree, the same Codex sessionID/thread remains
the same peer. Run `collab context` directly there; it reports the inherited
identity, liveness, tasks, inbox, `next_actions`, and master/authority state.
Never register the worktree as a second peer, promote yourself, or create a
second route.

## Master, peer and accepted goals

Master and peer both orchestrate their own accepted goals. Task routing,
design/review assignments and authority follow the [Collab Skill](../../collab/SKILL.md).
Reuse existing authorization; registration does not itself grant master authority.
Do not turn routine technical decisions or internal coordination into human approval.

Each owner registers its own accepted long-horizon goal when scheduling is needed.
Reuse the existing goal file and applicable cadence:

```sh
appsdk goal subscribe --goal <existing-goal.md> --interval <applicable-period>
```

Retain the real registration receipt. Board tasks, notes and generic timers do
not substitute for it; master must not register a peer's goal. Query
`appsdk goal status --json` only for a missing state. Ordinary registration does
not require a short-interval live replay. A scheduling change needs its own
armed, fired and consumed evidence. Continue ready work without waiting for a timer.
Finite goal/task schedules end with their lifecycle; the current master's
standing project patrol and peer cancellation follow
[board-idle patrol](../../collab/references/board-idle-patrol.md).

## Upstream AppSDK bug report

When a project hits a defect in AppSDK itself, do not patch around it or hide
it in project-local state:

```sh
appsdk bug list -q "<symptom>" --json --upstream
appsdk bug new --upstream -t "[SDK Bug] <symptom>" -m "<reproduction; expected; observed; evidence>" -l "<actual-priority>,appsdk"
```

Reuse an existing matching issue. Include the reproducible entry, first failing
layer and relevant evidence; add source/installed version only when needed to
explain the difference. Do not label every defect P0 or reread the full record
after a sufficient receipt. An upstream report does not prove local delivery.

`--upstream` is the explicit git-bug upstream route. The report must use the
actual symptom, reproduction, expected/observed result, version/commit,
and relevant logs; do not turn it into a local project bug or a fallback
workaround.

## Recovery and explicit reset

Use [Collab recovery](../../collab/references/stable-name-recovery.md) for
identity/route errors. Missing worktree-local state or an unreachable endpoint
does not prove that no master exists. Preserve errors and stop only affected work.

Only an explicitly authorized discard uses a reset. AppSDK governance reset
follows [bootstrap-migration](bootstrap-migration.md); Collab reset follows
[its current-master contract](../../collab/references/migration-daemon.md#reset-is-explicit-discard-not-recovery).
Neither resets the other's state or proves delivery. Do not delete journals,
socket/lock files, start another daemon or operate tmux to bypass failure.
