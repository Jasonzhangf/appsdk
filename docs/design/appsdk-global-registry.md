# AppSDK host-wide persistence and project registration

## Decision

AppSDK-owned host-wide state has one root: `~/.appsdk`. The root is resolved
from `HOME/.appsdk` in production. `APPSDK_HOME` is an explicit absolute-path
override for isolated tests and controlled sandboxes; it does not change the
production default and is never read from a project contract.

The first host-wide record is the append-only project registry:

```text
~/.appsdk/
  projects.jsonl
  projects.jsonl.lock
```

`projects.jsonl` is the source of truth. The lock file only serializes writers;
it is not a second state store. Future AppSDK-owned host-wide configuration,
migration receipts, and rebuildable indexes must use a named child of this
same root. A new global directory or platform-specific fallback requires a
separate contract change.

Release installation artifacts (`~/.cargo/bin/appsdk`, versioned bundles under
`~/.local/share/appsdk`, and the installed Skill link) are deployment outputs,
not runtime persistence. They remain managed by the official installer and are
verified separately; moving them into the registry would mix executable
delivery with mutable host state.

## Ownership boundary

| State | Owner | Durable location | Registration rule |
| --- | --- | --- | --- |
| Host-wide AppSDK project registry | AppSDK | `~/.appsdk/projects.jsonl` | `appsdk new` and `appsdk init` append or reuse one entry |
| Host-wide AppSDK configuration | Collab integration/its declared owner | `~/.appsdk/config.toml` when enabled | AppSDK forwards configuration; it does not create a shadow copy |
| Project governance contract | Managed project | `<project>/.appsdk/` | Committed project state; never copied into the host registry |
| Project run/control cache | Managed project | `<project>/.appsdk-control/` | Ignored local state; not host-wide registration truth |
| Collab daemon, journal, mailbox, claims, bindings | Collab | Collab's canonical state root | AppSDK never hand-edits or relocates it |
| Project-memory global index | project-memory | Its declared memory root | Independent subsystem; not an AppSDK registry or identity source |

The registry identifies a project root that opted into this AppSDK release. It
does not register an agent, grant `master`, establish a Collab route, or replace
the live runtime identity proof. A project can therefore have a valid registry
entry while its TUI route remains unbound or unavailable.

## Event contract

Each non-idempotent registration appends one JSON object and a terminating
newline:

```json
{
  "schema_version": 1,
  "event": "project.registered",
  "project_id": "project-<sha256-of-canonical-root>",
  "project_root": "/absolute/canonical/project/root",
  "sdk_version": "0.1.6",
  "registered_at": "2026-09-10T00:00:00Z",
  "source": "appsdk.init"
}
```

`project_id` is derived from the canonical root, so aliases and relative paths
resolve to the same project. Repeating registration for the same canonical
root and SDK version returns an `idempotent: true` receipt and appends nothing.
A later SDK version appends a new version event; history remains recoverable and
the latest matching event is the current registration view.

The writer takes an exclusive non-blocking lock, validates every existing line,
appends the event, and calls `sync_all`. Blank lines, malformed JSON, an
unsupported event shape, a symlinked registry path, or a busy lock fail closed.
The caller must preserve the exact error and may retry only through a deliberate
operator action; there is no tight retry loop or silent fallback.

## Initialization ordering

`appsdk init` and `appsdk new` use the same order:

```text
resolve project root
  -> create only the empty target directory when `new` needs it
  -> register in ~/.appsdk/projects.jsonl
  -> emit the registration receipt
  -> write project governance scaffold
  -> attempt one optional Collab bootstrap
```

Registration failure stops before project governance files are written. A
successful registration does not make optional Collab bootstrap failure look
successful. The command prints a machine-readable `appsdk-registration` line
containing the registry path, canonical project root, project ID, SDK version,
and idempotency flag.

## Recovery and migration

The registry is host-wide AppSDK state, so migration snapshots record its path,
line count, last event ID/digest, and the exact binary version. They do not copy
credentials or runtime tokens. Existing project `.appsdk/` contracts,
`.appsdk-control/` caches, Collab journal/mailbox, and project-memory sources
follow their own owners and migration contracts. A missing registry is
recreated by the next successful `init`/`new`; a malformed registry is retained
and reported until repaired by an explicit migration owner.

Removing a stale registry entry is not part of ordinary initialization. It
requires a named migration/reset plan, an immutable snapshot, an authorized
canonical command, and a post-action replay showing that every retained project
can register again. No broad `rm`, truncation, or manual JSON rewrite is valid.

## Verification

The minimum evidence for this feature is:

1. A clean candidate from `origin/main` passes formatter, focused registry
   tests, CLI smoke, and release build.
2. `appsdk new <root>` creates exactly one event under the injected test root;
   a following `appsdk init <root>` is idempotent.
3. Two canonical project roots produce two project IDs and two events.
4. A malformed JSONL or busy lock returns a stable `GLOBAL_REGISTRY_*` error
   and writes no project scaffold.
5. The merged mainline binary and installed binary emit the same receipt shape.

This evidence proves host registration only. It does not prove Collab daemon
health, TUI identity, or bidirectional communication; those remain separate
gates owned by Collab and the migration workflow.
