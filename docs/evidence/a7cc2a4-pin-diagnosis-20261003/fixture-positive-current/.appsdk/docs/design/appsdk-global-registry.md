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
  runtimes.jsonl
  runtimes.jsonl.lock
  communication.jsonl
  communication.jsonl.lock
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
| Host-wide communication discovery | AppSDK communication owner | `~/.appsdk/communication.jsonl` | Scope/agent registration appends or reuses one address owner |
| Project governance contract | Managed project | `<project>/.appsdk/` | Committed project state; never copied into the host registry |
| Project run/control cache | Managed project | `<project>/.appsdk-control/` | Ignored local state; not host-wide registration truth |
| Collab daemon, journal, mailbox, claims, bindings | Collab | Collab's canonical state root | AppSDK never hand-edits or relocates it |
| Project-memory global index | project-memory | Its declared memory root | Independent subsystem; not an AppSDK registry or identity source |

The registry identifies a project root that opted into this AppSDK release. It
does not register an agent, grant `master`, establish a Collab route, or replace
the live runtime identity proof. A project can therefore have a valid registry
entry while its TUI route remains unbound or unavailable.

The host runtime registry is the second append-only stream under the same root.
`runtimes.jsonl` binds a stable `runtimeId` to one App Server endpoint,
namespace, project root, declared capabilities, process id and a derived
fingerprint. `appsdk communication ... register_runtime`
appends or reuses one binding. A later registration with the same ID but a
changed endpoint, cwd or namespace fails with `GLOBAL_RUNTIME_IDENTITY_CONFLICT`;
it never overwrites the old record. Capability changes and volatile process or
App Server fields use an append-only `runtime.refreshed` record while preserving the
stable transport identity. Capability-bearing records use a versioned
length-prefixed encoding for every identity field and capability item, so
capability bytes cannot change field boundaries. Legacy records are not
migrated or replayed by the current AppServer-only registry; an explicitly
authorized runtime-registry reset archives them and rebuilds the empty baseline.
A scope or agent may be
created only when its `runtimeId` resolves to this exact binding. The registry
is an identity binding and replay source; it does not claim that an appserver
delivered a message. The host must report that fact through the communication
`record_delivery` operation.

The communication discovery registry is the third host-wide stream. It maps a
`scopeId/sessionId` address to the canonical project root whose
`.appsdk-control/communication/mailbox.jsonl` owns the complete scope and agent
record. It contains no role, lease, parent, route, message, or delivery state;
the target project mailbox remains the only source for those facts. A sender
uses the mapping only to locate the target mailbox, replays its identity events,
and then applies the existing route policy. Cross-project discovery therefore
works with separate project mailboxes without turning the host index into a
second authority.

`communication.jsonl` accepts `communication.scope.registered`,
`communication.agent.registered`, and `communication.agent.rebound` events.
Addresses and project roots are canonical and length-bounded. Rebinding leaves
an old-address tombstone; lookup reports the new address, while a send to the
old address fails with `agent_address_rebound` rather than silently rewriting
the caller's request. Scope or address conflicts, malformed lines, unknown
events, missing final newlines, symlinked paths, and a busy lock fail closed.
Registration is idempotent only when the address and canonical project owner
are unchanged.

Communication registration is recoverable across the project mailbox and this
host projection. The project first records a `discovery.pending` intent with the
complete scope, agent, or rebind record, then commits the local mailbox fact,
publishes the host event, and finally records `discovery.reconciled`. A process
stop or write error between those stores leaves the intent and the original
error; the next normal mailbox open retries from that intent. The host stream
never becomes an uncommitted source of truth, and repair never edits or copies
historical JSONL by hand.

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

The writer takes an exclusive lock, validates every existing line, appends the
event, and calls `sync_all`. Project initialization waits up to 30 seconds with
bounded backoff when another AppSDK initialization holds the writer lock; this
only handles expected contention and never changes the event or project root.
Blank lines, malformed JSON, an unsupported event shape, a symlinked registry
path, lock I/O failures, or contention that exceeds the deadline fail closed
with the original `GLOBAL_REGISTRY_*` error. There is no unbounded retry or
silent fallback.

## Initialization ordering

`appsdk new` uses the fail-closed two-phase order:

```text
resolve project root
  -> create only the empty target directory when `new` needs it
  -> validate and reserve ~/.appsdk/projects.jsonl (hold its writer lock)
  -> write project governance scaffold
  -> append the registration event and emit the receipt
  -> attempt one optional Collab bootstrap
```

Registry validation or reservation failure stops `appsdk new` before project
governance files are written. The reservation keeps competing writers out
until the local initialization transaction has completed; dropping it on any
local failure publishes no project event. A final append or sync failure
remains an explicit `GLOBAL_REGISTRY_*` error and never becomes a successful
initialization. A successful registration does not make optional Collab
bootstrap failure look successful. The command prints a machine-readable
`appsdk-registration` line containing the registry path, canonical project
root, project ID, SDK version, and idempotency flag.

`appsdk init` (including fresh reset and idempotent SDK refresh) treats global
registration as an auxiliary capability. It completes local governance first,
then attempts registration:

- On success it prints the same `appsdk-registration` receipt.
- On any `GLOBAL_REGISTRY_*` failure it prints
  `GLOBAL_PROJECT_REGISTRATION_PENDING:<exact error>` to stderr and still
  succeeds locally. The error is surfaced verbatim; nothing is written to
  `projects.jsonl`, so a failed or unknown registry is never reported as a
  successful registration and no partial event is fabricated.

Local governance (project contract, maps, records, Active/Protected state,
`sdk.lock`) does not depend on the host registry, so a damaged or busy
registry must not freeze independent development or recovery. The boundary
stays explicit: operations that consume global identity, shared ownership, or
cross-project coordination must check the registration capability
separately and remain blocked until a real registration receipt exists.
`GLOBAL_PROJECT_REGISTRATION_PENDING` means exactly "local work may continue,
host registration is not established".

## Recovery and migration

The registry is host-wide AppSDK state, so migration snapshots record its path,
line count, last event ID/digest, and the exact binary version. They do not copy
credentials or runtime tokens. Existing project `.appsdk/` contracts,
`.appsdk-control/` caches, Collab journal/mailbox, and project-memory sources
follow their own owners and migration contracts. A missing registry is
recreated by the next successful `init`/`new`; a malformed registry is retained
and reported until repaired by an explicit migration owner. The same fail-closed
rule applies to `runtimes.jsonl`: a missing runtime record blocks communication
registration while leaving ordinary project governance usable.

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
