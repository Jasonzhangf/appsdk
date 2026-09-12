# AppSDK M1 identity and host registry closeout

## Objective

Make the AppSDK source repository and every managed project use one explicit
host-wide AppSDK persistence root (`~/.appsdk`), while keeping project state and
Collab state in their existing owners and proving that `new`/`init` register a
canonical project exactly once per SDK version.

## Baseline and target

- Baseline: `origin/main` at `a4f19529b524398441a5d38ea2e2e8df2336d09f`;
  AppSDK has no host project registry and initialization has no registration
  receipt.
- Target: the reviewed M1 candidate from this worktree, with
  `~/.appsdk/projects.jsonl` as the AppSDK host registry, idempotent canonical
  registration in `appsdk new` and `appsdk init`, and explicit persistence and
  migration boundaries.
- Runtime identity target: Collab's single global daemon and its live TUI route
  remain the authority for agent identity and communication. This goal does
  not invent a second daemon or an AppSDK identity registry.

## Owner and boundaries

The AppSDK M1 owner controls this candidate worktree and the following paths:

```text
rust/src/global_registry.rs
rust/src/main.rs
rust/tests/cli_smoke.rs
contracts/sdk-bundle.manifest.json
docs/design/appsdk-global-registry.md
docs/goals/appsdk-m1-identity-migration-20260910-plan.md
docs/migrations/appsdk-m1-identity-migration-20260910-inventory.md
```

The owner may update the AppSDK migration and governance Skill files already
admitted on this branch. It must not modify the dirty source root, `codexapp`,
the Collab source repository, Collab journal/mailbox/token/task/claim/binding,
other project worktrees, or installation paths during candidate development.

## Loop contract

Every M1 round has the five required parts and uses this order:

```text
Discover -> Hand off -> Verify -> Persist -> Schedule
```

| Part | M1 implementation |
| --- | --- |
| Trigger | `appsdk new` or `appsdk init`, plus the explicit migration owner invoking the goal |
| Work | Resolve the project root, validate the host registry, append/reuse a registration, and emit a receipt |
| Gate | Formatter, focused Rust tests, CLI registration replay, bundle consistency, full relevant tests, and independent review |
| State | Append-only `~/.appsdk/projects.jsonl`; candidate evidence and migration inventory in the named docs/run note |
| Stop | Stop on malformed registry, symlink, lock contention, identity mismatch, failed gate, unknown migration state, or any forbidden-path write; hard stop after the listed gates pass |

## Acceptance criteria

1. The production default host root is `HOME/.appsdk`; an explicit absolute
   `APPSDK_HOME` is available only for isolated tests/sandboxes.
2. `appsdk new <project>` creates one `project.registered` JSONL event and
   prints an `appsdk-registration` receipt containing canonical root, project
   ID, registry path, SDK version, and `idempotent: false`.
3. A subsequent `appsdk init <project>` returns a successful idempotent receipt
   and does not append a duplicate event. A later SDK version may append a new
   version event without rewriting history.
4. Relative aliases resolve to the same canonical project identity; distinct
   canonical roots receive distinct IDs.
5. Malformed/blank registry lines, symlinked registry objects, invalid project
   roots, or a busy lock fail closed before project scaffold writes and expose a
   stable `GLOBAL_REGISTRY_*` error.
6. The bundle manifest and embedded resource table contain the registry design
   and migration Skill with matching paths.
7. Existing project `.appsdk/`, `.appsdk-control/`, business data, and
   Collab/project-memory state remain under their declared owners. No token or
   copied runtime identity enters the host registry.
8. Candidate and merged mainline gates are run separately. Installation,
   daemon restart, live TUI route proof, and deployed replay remain separate
   post-merge gates; a passing source test does not claim those states.

## Migration and failure policy

Use the migration Skill's `inspect -> classify -> snapshot -> freeze ->
install/restart -> identity-rebind -> verify -> resume` procedure for existing
host state. Preserve old registry/config and all Collab/project evidence until a
path-level inventory and authorized canonical migration/reset exist. Never
truncate JSONL, start a second daemon, copy tokens, or retry a timeout in a
10-second loop. An `in_progress` or `unknown` result remains visible and stops
dependent writes.

## Verification commands

```text
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo test --manifest-path rust/Cargo.toml --bin appsdk global_registry --quiet
cargo test --manifest-path rust/Cargo.toml --test cli_smoke project_creation_and_initialization_persist_host_registration
cargo test --manifest-path rust/Cargo.toml --test communication_cli --quiet
cargo check --manifest-path rust/Cargo.toml --all-targets
cargo build --manifest-path rust/Cargo.toml
git diff --check
```

After review and clean-main integration, run the repository's official install,
then verify the installed version/hash and the separately owned Collab daemon
restart and live TUI two-way replay. Record each receipt independently.

## Non-goals

- No Collab daemon, route, session, appserver, or mailbox implementation.
- No Desktop goal subscription or Desktop runtime registration.
- No relocation of Collab or project-memory data into the AppSDK registry.
- No deletion of legacy journal/mailbox/token/task/claim/binding or unrelated
  project state.
- No broad refactor of the communication prototype in this M1.
