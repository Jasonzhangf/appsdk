# Collab identity context: daemon ownership and approved recovery contract

## 1. Status, authority, and boundaries

This document is the D2-B design contract for `collab context`. It freezes the
public invocation schemas, daemon owner decisions, operation phases, restart
readback, and failure terminals. It does not assert that any missing phase,
receipt, query, or recovery seam already exists. It also does not implement
them and does not authorize product code.

Authority order:

1. The user's four requested Collab capabilities as persisted in the canonical
   task Goal at
   `/Users/fanzhang/Documents/github/appsdk/docs/goals/collab-context-identity-peer-crud-remediation-20261008.md`.
   The candidate worktree keeps a byte-for-byte review copy at
   `docs/goals/collab-context-identity-peer-crud-remediation-20261008.md`;
   both copies must have the same SHA-256 before a design review. The exact
   source SHA-256 for this correction is
   `1e266b371e7ba296d3431fc8a4a8eb5ef09b754219f78da519b8cad57c672aab`.
2. Accepted bounded plan and acceptance
   `plan-resume-20261009.md` /
   `plan-resume-20261009-acceptance.md`.
3. Accepted D2-A source map
   `d2-admission-receipt-map-20261009.md` and its acceptance.
4. This D2-B contract for invocation, approval, phase, result, and query
   semantics.
5. `collab-anchor-restore-model.md` for automatic anchor recovery and its
   recovery ladder.
6. The master authority contract for grant state and grant changes.

If a lower document conflicts with a higher one, the higher source wins. A
conflict that cannot be resolved inside the four D2-B design files is a
`BLOCKED` design condition, not permission to invent a product owner.

Scope boundary:

- A6 Create remains `BLOCKED`; this document does not define or admit it.
- D3-RUC, peer Create/Update/Close, product code, daemon writes, identity
  operations, and live tests are out of scope.
- A design-only acceptance of this document means `READY_FOR_DESIGN_REVIEW`,
  never `PASS`, implementation admission, or product completion.

## 2. Terms and exact owner map

| Term | Exact meaning | Owner |
| --- | --- | --- |
| caller operation key | Caller-generated `operation_id` retained before the first side effect; it is the outer identity-operation identity. | CLI/MCP for generation and retention; daemon for durable admission and conflict checking |
| invocation | One of the mutually exclusive context request intents: automatic context/recovery, fact supplement, approved identity recovery, or operation query. | CLI/MCP parses; daemon `identity_gate` decides |
| intent digest | Canonical hash over project/app scope, target identity, action, normalized intent fields, the expected incumbent binding/generation or grant id/generation, and approval decision/action when present. It excludes the digest field itself and descriptive approval metadata (`decided_by`, `approved_at_ms`). | shared normalizer; daemon verifies |
| phase | Durable outer operation progress. This is not a promise that a later side effect committed. | host-local identity operation owner |
| receipt | An authoritative committed command or owner record. A nested Register receipt is not an outer operation receipt. | producing owner |
| readback | Pure projection of durable outer phases plus current owner facts. It never repairs state. | identity_gate query projection |
| approval | Explicit user decision for one exact target and one exact transaction. It is not endpoint, token, scope, or current-identity proof. | caller submits; daemon validates scope and owner conditions |

The existing `ProjectRuntimeManager::identity_context` remains the only
bootstrap identity orchestrator. The existing Register transaction remains the
binding/route transaction. Identity resolution remains owned by the resolver.
The local credential file remains a daemon-written cache of the committed
identity, not a separate authority. The authority owner remains the owner of
master grant state and explicit grant changes.

`actor_binding_id` is always the real committed binding selected by the daemon.
No layer may fabricate one. An approved recovery uses an internal admission
proof to enter the existing Register transaction when ordinary token admission
would reject a stale incumbent.

## 3. One invocation, one discriminated result

Every public context response uses the same envelope. `operation_id` is present
for all four invocations. `invocation` and `action` are separate: `invocation`
selects the lifecycle, while `action` names the requested business operation.
The query selector uses `invocation: "query"`; it never reuses the original
mutation action as if it were a query action.

The query request's `action` is always the selector value `query`. The query
capability, rather than caller-repeated target/action/digest claims, proves
access to the retained outer operation. Cancellation uses the operation
owner's cancellation handle attached to the active invocation; it is not a
separate context invocation, CLI command, or MCP argument.

For every mutating invocation, the client creates and atomically stores its
random `query_capability` with the operation key before the first send, then
includes that capability in the typed daemon request. At admission the daemon
immediately hashes it and persists only the hash with the operation record;
the raw value is never logged or returned. Query sends the locally retained
capability and compares it with that stored hash. A missing capability on an
initial mutating request is rejected before side effects.

```json
{
  "ok": false,
  "result": {
    "operation_id": "ctxop-<opaque>",
    "invocation": "automatic | supplement | approved_recovery | query",
    "action": "context | restore_identity | replace_binding | replace_master_grant | query",
    "phase": null,
    "outcome": "missing_facts | approval_required | denied | partial | unknown | failed | completed | cancelled",
    "committed_phases": [],
    "failed_phase": null,
    "requires": {
      "kind": null,
      "fields": [],
      "sources": {},
      "approval": null,
      "repair_invocation": null
    },
    "snapshot": null,
    "owner_readback": {},
    "queried_operation": null
  }
}
```

Required invariants:

- `ok` means the requested operation completed. It is true only for
  `outcome: "completed"`; it is false for `missing_facts`, `approval_required`,
  `denied`, `failed`, `partial`, `unknown`, and `cancelled`. A query whose
  projection was returned has `outcome: "completed"`, `ok: true`, and carries
  the queried operation's outcome separately in `queried_operation`.
- `phase` is the last durable phase the daemon can prove. It is null when no
  operation was admitted and on the query invocation envelope. It is not
  inferred from elapsed time, logs, liveness, or process state.
- `committed_phases` lists only phases with a durable producer receipt.
- `admitted` is an admission marker, not a committed business phase. It never
  appears in `committed_phases`; an admitted outer operation is represented by
  a durable operation record and `phase: "admitted"`.
- `failed_phase` is non-null when a phase failed after durable admission.
- `snapshot` is present only when the identity context snapshot is complete and
  safe to project. It never contains token or credential secret material.
- `owner_readback` contains owner-specific readback facts. It is a projection,
  not permission to mutate the owner resource.
- `requires.repair_invocation` is advice for a separate explicit invocation. It
  is never executed by automatic retry or by query.

### 3.1 Outcome transition table

| invocation / condition | Pre-commit terminal | Post-commit terminal | Queryable after restart |
| --- | --- | --- | --- |
| automatic context, facts complete | `completed` | `partial` if a later phase is incomplete; `failed` if a terminal error occurs after a committed phase; `unknown` if an applicable journal cannot be read | yes for `partial`, `unknown`, and post-commit `failed` |
| automatic context, facts incomplete | `missing_facts` | not applicable | no mutable state; a new invocation may supply facts |
| supplement, facts valid | `completed` | `partial` or post-commit `failed` under the same phase rules as automatic | yes after admission |
| supplement, facts invalid/conflicting | `denied` or `failed` before admission | not applicable | only if an admitted operation record exists |
| approved recovery, exact approval valid | `completed` | `partial`/`unknown`/post-commit `failed`; committed phases are retained | yes |
| approved recovery, missing/ambiguous/stale approval | `approval_required` or `denied` before admission | not applicable | no side-effect replay |
| grant replacement as a distinct owner phase | `completed` for the owner phase | `partial` when identity/binding committed but authority owner did not complete; `unknown` if authority journal unreadable | yes when the outer operation was admitted |
| query | pure readback | `completed` query projection containing queried-operation `completed`, `partial`, `failed`, or `unknown` | yes, subject to query capability |
| cancellation | only at an enumerated safe boundary | `cancelled` with committed phases preserved | yes if admitted |

`approved` is not an outcome. It is a phase name (`approval_decision`) and an
internal admission state. Approval alone never means completion.

`cancelled` is allowed only at these safe boundaries:

1. before durable outer admission;
2. after `admitted` but before the first identity/binding/route/credential/grant
   mutation;
3. after a phase producer has committed and before the next phase begins,
   provided the next phase has no durable in-flight intent.

Once a side-effecting owner transaction has begun, the operation cannot report
`cancelled`; it must report `partial`, `unknown`, or `failed` with the committed
phases retained.

If the caller cancels before durable admission, no daemon operation record or
query result exists. CLI returns a local typed `cancelled` envelope with
`phase: null` and no committed phases; MCP terminates the active `tools/call`
through `notifications/cancelled` and has no tool result to parse. After
admission, cancellation uses the durable operation result envelope and remains
queryable by the pre-dispatch operation key/capability.

### 3.2 Public examples

Automatic context with all facts observable:

```json
{
  "ok": true,
  "result": {
    "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T3",
    "invocation": "automatic",
    "action": "context",
    "phase": "context_complete",
    "outcome": "completed",
    "committed_phases": ["nested_register", "route", "credential", "lease", "context_complete"],
    "failed_phase": null,
    "requires": {"kind": null, "fields": [], "sources": {}, "approval": null, "repair_invocation": null},
    "snapshot": {"registered": true},
    "owner_readback": {},
    "queried_operation": null
  }
}
```

Missing facts:

```json
{
  "ok": false,
  "result": {
    "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T4",
    "invocation": "automatic",
    "action": "context",
    "phase": null,
    "outcome": "missing_facts",
    "committed_phases": [],
    "failed_phase": null,
    "requires": {
      "kind": "identity_facts",
      "fields": ["session_id", "thread_id"],
      "sources": {"session_id": "transcript metadata", "thread_id": "current native thread"},
      "approval": null,
      "repair_invocation": "collab context --provide '<JSON containing exactly the required fields>'"
    },
    "queried_operation": null,
    "snapshot": null,
    "owner_readback": {}
  }
}
```

`missing_facts` is returned before outer admission: the supplied operation key
is not persisted by the daemon and there are no committed phases. The returned
supplement command starts a fresh invocation with a fresh key generated and
retained by the CLI. Repeating the original automatic request is also
side-effect-free. A supplement never reuses an unadmitted key as a different
intent.

Partial after Register, with route/credential incomplete:

```json
{
  "ok": false,
  "result": {
    "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T5",
    "invocation": "automatic",
    "action": "context",
    "phase": "nested_register",
    "outcome": "partial",
    "committed_phases": ["nested_register"],
    "failed_phase": "route",
    "requires": {
      "kind": "repair",
      "fields": [],
      "sources": {},
      "approval": null,
      "repair_invocation": "collab context --op <new-operation-id>"
    },
    "snapshot": null,
    "owner_readback": {"route": {"state": "unknown"}},
    "queried_operation": null
  }
}
```

Query response is a pure read projection:

```json
{
  "ok": true,
  "result": {
    "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T6",
    "invocation": "query",
    "action": "query",
    "phase": null,
    "outcome": "completed",
    "committed_phases": [],
    "failed_phase": null,
    "queried_operation": {
      "phase": "route",
      "outcome": "partial",
      "committed_phases": ["nested_register", "route"],
      "failed_phase": "credential",
      "requires": {
        "kind": "repair",
        "fields": [],
        "sources": {},
        "approval": null,
        "repair_invocation": "collab context --approve-identity '<new exact approval object>'"
      },
      "owner_readback": {"credential": {"state": "unknown"}}
    },
    "requires": {"kind": null, "fields": [], "sources": {}, "approval": null, "repair_invocation": null},
    "snapshot": null,
    "owner_readback": {}
  }
}
```

The query response above does not perform the repair. The suggested invocation
is a separate explicit `collab context` request with its own operation key,
approval, and intent. A query caller that lacks the original key or the read
capability gets `denied`; it does not get a weaker proof.

## 4. Stable operation identity and proof

### 4.1 Generation and retention

Before any request that can mutate identity state is sent, the client generates
or accepts an operation key and generates a random 256-bit query capability.
The CLI atomically persists `{operation_id, query_capability,
project_scope, app_scope_id}` with mode `0600` under
`~/.collab/context-operations/<operation_id>.json`, then sends the request. The
shared CLI/MCP client helper owns this local proof record; it contains no token,
credential, or approval secret. A caller-supplied `--op <id>` is validated and
must not overwrite a record with different scope or capability. If `--op` is
omitted, the CLI generates and persists the key before send; it does not rely on
the response to retain it.

The MCP adapter uses the same helper and store. It generates/persists the key
and capability before daemon dispatch when omitted, and includes the key in
the typed result. A caller that supplies an operation id must supply a matching
local proof record or let the adapter create one before the first send.

The daemon persists an outer operation record before any identity, binding,
route, credential, grant, or lease side effect. The record contains:

- outer `operation_id`;
- a hash of the query capability (the capability itself remains client-local);
- canonical `project_scope` and `app_scope_id`;
- `target_identity`;
- `action`;
- `intent_digest`;
- expected incumbent binding id/endpoint generation or grant id/generation
  when the corresponding owner resource is being replaced;
- `invocation`;
- phase and committed phase set;
- nested Register `command_id` and `operation_id` before dispatch, when this
  operation needs a nested Register;
- approval metadata, if present; never token or credential material.

The client proof record has exactly this non-secret schema; it is created with
an atomic exclusive write before the request is sent and is never overwritten
for an existing operation key:

```json
{
  "version": 1,
  "operation_id": "ctxop-<opaque>",
  "query_capability": "base64url:<32-random-bytes>",
  "project_scope": "/canonical/project",
  "app_scope_id": "appserver-cli"
}
```

### 4.2 Reuse and conflict

- Same key and same normalized intent: the daemon returns a representation of
  the existing durable operation. It does not repeat a side effect.
- Same key and different normalized intent: reject with
  `IDENTITY_OPERATION_INTENT_CONFLICT` before any side effect.
- A `missing_facts` response occurs before admission and creates no daemon
  operation record. Its submit template starts a fresh supplement operation
  with a fresh key; it cannot reuse the unadmitted key as a new intent.
- A different project/app scope, target, action, or intent digest cannot use
  the retained key to read or replace the operation.
- A caller can query a lost response with the same key. It does not need the
  retired old token.

### 4.3 Query proof

The public query carries the retained outer key. The CLI/MCP helper loads the
matching query capability from its local proof record and sends it to the
daemon. The daemon compares the capability hash, project/app scope and key
against the durable operation record; target, action and intent digest are
read from that record and returned only in the projection, not accepted as
caller assertions. Missing or mismatched local proof is denied. The key and
capability are created and persisted before the original request is sent, so
query remains possible after a lost response or adapter restart.

Query authorization uses the same host-local Unix socket and same canonical
project/app scope. It does not require the retired identity token and does not
call `Req::Context`, which remains an ordinary authenticated read requiring
`worker_id + token`. The read capability is not a mutation credential; it can
only read the durable projection. A query never continues or repairs effects.

## 5. Approved stale-credential recovery seam

### 5.1 Authority facts

The authoritative identity facts are the committed project reducer binding,
worker record, and credential held by the daemon registration owner. The local
`identities/<worker>/identity.json` file is a cache and a durable local receipt.
A stale local receipt does not authorize a replacement and cannot be treated as
the current daemon registration.

If the local receipt is stale but the committed project reducer has a binding
for the same target and scope, the daemon first treats the reducer record as the
incumbent. If the committed binding was retired or is demonstrably absent, the
approved recovery may create a replacement binding only through the explicit
approved transaction below. It never guesses by recency, liveness, or pane
name.

### 5.2 Ordinary admission remains intact

Ordinary `Req::Register` and ordinary authenticated commands keep their
existing token, binding, generation, scope, and runtime checks. A mismatch in
an ordinary command remains a typed rejection, including `TOKEN_MISMATCH` when
the supplied credential is rejected. The approved recovery transaction is the
only exception, and it is available only through the explicit approval schema.

### 5.3 Approved replacement transaction

The daemon owns one internal transaction for an approved stale-credential
recovery. It runs after scope validation and before the ordinary Register
admission that would otherwise reject the stale incumbent:

1. **Retain the outer operation.** Persist `admitted` and the normalized intent
   before touching identity, binding, route, credential, grant, or lease state.
2. **Validate approval.** Require a non-empty approval decision bound to the
   exact project scope, app scope, target identity, action, normalized intent
   digest, expected incumbent binding id, and expected incumbent generation.
   Reject a changed incumbent with `APPROVAL_STALE_CONFLICT`.
3. **Verify endpoint ownership.** Re-run the existing adapter probe against the
   candidate endpoint/session/thread. Approval does not replace this proof.
4. **Select incumbent.** Read authoritative committed identity facts from the
   project reducer/worker owner and credential owner. The stale local receipt is
   evidence only; it cannot be promoted to authority. If no authoritative
   incumbent exists and the user approval names a target, the approved
   replacement may create the replacement through the same canonical target
   constraints.
5. **Persist approval decision.** Write the approved decision and exact target
   to the outer operation record. Persist `approval_decision` before dispatch.
6. **Retire and rebind.** Build the replacement binding for the real selected
   runtime. Retire the old binding/generation and publish the replacement in
   the existing Register/binding owner transaction. The replacement retires
   only the exact approved incumbent; it does not clear another project’s
   binding. The old binding cannot remain a valid route owner after commit.
7. **Internal Register admission proof.** The identity owner passes a
   daemon-issued admission proof bound to the outer operation and approval
   decision into the existing Register transaction. The proof authorizes only
   this approved replacement. It is not a token, not a user-visible credential,
   and never substitutes a fabricated `actor_binding_id`. The Register owner
   still creates/uses the real binding id it commits.
8. **Persist phase and receipt.** The outer record stores the nested Register
   `command_id`, `operation_id`, binding id, and endpoint generation before the
   dispatch returns. The existing command receipt remains owned by the existing
   receipt producer. The outer owner records the reference and phase; it does
   not counterfeit the inner receipt.
9. **Route and credential.** Route publication remains the route owner’s
   transaction. Credential persistence remains the credential owner’s atomic
   file write after Register success. Failure after Register leaves the
   committed phases and reports `partial`, `unknown`, or post-commit `failed`.
10. **Grant.** A recovered master identity does not implicitly grant, clear,
    or replace authority. Same-principal grant reissue remains the existing
    Register transaction behavior when its exact match conditions are met. An
    explicit grant replacement is a separate owner phase with its own approval
    and its own outer intent.

### 5.4 Incumbent conflict fence

The approval request must include `expected_incumbent`:

```json
{
  "binding_id": "binding-<id>",
  "endpoint_generation": 7,
  "target_identity": "worker-<id>",
  "project_scope": "/canonical/project",
  "app_scope_id": "appserver-cli"
}
```

Before commit, the daemon reloads the exact incumbent and compares binding id,
generation, target, project scope, and app scope. If any value differs, it
returns `APPROVAL_STALE_CONFLICT` with the observed values and no mutation.
Approval never authorizes a later different incumbent.

## 6. Public request schemas

### 6.1 CLI argv

Automatic context/recovery:

```text
collab context \
  [--op <operation-id>] \
  [--project <canonical-project-root>] \
  [--app-scope <app-scope-id>]
```

If `--op` is omitted, the CLI creates and retains the operation key and query
capability before it contacts the daemon, and sends that capability with the
first request so the daemon can bind the hash during admission.

Fact supplement:

```text
collab context \
  --provide '<JSON object with only required_fields>'
```

The CLI generates a fresh operation key for this new invocation. The
`missing_facts` response's original key was never admitted and must not be
reused with a different intent.

Approved identity recovery/rebinding:

```text
collab context \
  [--op <operation-id>] \
  --approve-identity '<JSON approval object>' \
  [--provide '<JSON object with only required_fields>']
```

Separately approved master grant change (submitted in the same invocation only
as a second owner phase):

```text
collab context \
  [--op <operation-id>] \
  --approve-identity '<JSON approval object>' \
  --approve-grant '<JSON approval object>'
```

Operation query:

```text
collab context \
  --op <retained-operation-id> \
  --query
```

`--query` loads the matching local proof record from
`~/.collab/context-operations/<operation-id>.json`; the CLI does not ask the
agent to reconstruct a target, action, or digest.

There is no `--restore-as`, `--worker`, `--token`, or generic `--force`
argument. `--approve-identity` and `--approve-grant` are distinct flags; one
flag cannot silently authorize the other owner.

The CLI exits 0 only for `ok: true`. It exits 2 for a typed incomplete or
refused result (`missing_facts`, `approval_required`, `denied`, `partial`,
`unknown`, `failed`, or `cancelled`) while preserving the full JSON result on
stdout. Invalid argv/schema exits 64 and has no result envelope. The MCP adapter
sets `isError: false` whenever it returns a valid typed result envelope,
including `ok: false` outcomes; it uses `isError: true` only when no typed
result can be returned (transport/protocol failure). For CLI stdout, inspect
`$.ok`, `$.result.outcome`, and, for a query, `$.result.queried_operation.outcome`.
For MCP, the JSON-RPC tool response has `$.result.isError` and
`$.result.content[0].text`; parse that text as the same CLI payload and inspect
those same payload paths. The current adapter does not return `structuredContent`.
Never equate `isError: false` with completion.

The identity operation owner attaches one cancellation handle to each active
invocation. CLI SIGINT and MCP `notifications/cancelled` for the active
`tools/call` request signal that handle; they do not create another Collab
command or operation. The owner checks cancellation only at the safe boundaries
in §3.1; once a side-effecting owner transaction has begun, it returns the
current `partial`/`unknown` result or completes the in-flight owner phase, but
never claims `cancelled`. Cancellation does not roll back committed phases.
The operation key and capability are retained in the adapter's local proof
store before dispatch, so a lost response can be queried after adapter restart.

### 6.2 Approval JSON

```json
{
  "decision": "approved",
  "decided_by": "user",
  "target_identity": "worker-<id>",
  "project_scope": "/canonical/project",
  "app_scope_id": "appserver-cli",
  "action": "restore_identity",
  "expected_incumbent": {
    "binding_id": "binding-<id>",
    "endpoint_generation": 7
  },
  "intent_digest": "sha256:<hex computed without this field or descriptive approval metadata>",
  "approved_at_ms": 1770000000000
}
```

`action` is `restore_identity` or `replace_binding` for identity approval. Its
`expected_incumbent` binds the exact binding id and endpoint generation.
`replace_master_grant` has a separate schema and owner fence:

```json
{
  "decision": "approved",
  "decided_by": "user",
  "target_identity": "master-<id>",
  "project_scope": "/canonical/project",
  "app_scope_id": "appserver-cli",
  "action": "replace_master_grant",
  "expected_grant": {"grant_id": "grant-<id>", "generation": 3},
  "intent_digest": "sha256:<hex computed without this field or descriptive approval metadata>",
  "approved_at_ms": 1770000000000
}
```

The normalizer hashes decision/action, scope, target, the resource-specific
fence, and requested facts, excluding `intent_digest` itself, `decided_by`, and
`approved_at_ms`; this avoids a self-referential digest. `decided_by` is
descriptive metadata; the daemon validates the approval fields and scope, not
the caller’s claim of identity. Approval text or a truthy boolean alone is
insufficient. Identity approval cannot substitute a grant fence and grant
approval cannot substitute an incumbent binding fence.

### 6.3 MCP `tools/call`

`collab_context` arguments retain the existing `provide` field for fact
supplement and add `project_scope`, `app_scope_id`, `operation_id`,
`approve_identity`, `approve_grant`, `query`, and the private client-to-daemon
`query_capability` transport field. The MCP adapter uses the same
durable local proof store as the CLI;
when `operation_id` is omitted, it creates and persists the key and query
capability before daemon dispatch, then sends the capability to the daemon on
the initial mutating invocation. The field is never exposed as an MCP argument
or included in caller-facing output.

Automatic context:

```json
{"name":"collab_context","arguments":{"project_scope":"/canonical/project","app_scope_id":"appserver-cli"}}
```

```json
{
  "name": "collab_context",
  "arguments": {
    "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T7",
    "provide": {"session_id": "session-1", "thread_id": "thread-1"}
  }
}
```

```json
{
  "name": "collab_context",
  "arguments": {
    "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T8",
    "approve_identity": {
      "decision": "approved",
      "target_identity": "worker-a",
      "project_scope": "/canonical/project",
      "app_scope_id": "appserver-cli",
      "action": "replace_binding",
      "expected_incumbent": {"binding_id": "binding-7", "endpoint_generation": 7},
      "intent_digest": "sha256:..."
    }
  }
}
```

```json
{
  "name": "collab_context",
  "arguments": {
    "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T9",
    "approve_grant": {
      "decision": "approved",
      "target_identity": "master-a",
      "project_scope": "/canonical/project",
      "app_scope_id": "appserver-cli",
      "action": "replace_master_grant",
      "expected_grant": {"grant_id": "grant-3", "generation": 3},
      "intent_digest": "sha256:..."
    }
  }
}
```

```json
{
  "name": "collab_context",
  "arguments": {
    "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T10",
    "query": true
  }
}
```

### 6.4 Daemon request JSON

The daemon request is an extension of `Req::IdentityContext`. The outer
envelope is typed and cannot contain unknown fields:

```json
{
  "op": "IdentityContext",
  "project_context": {
    "app_scope_id": "appserver-cli",
    "project_scope": "/canonical/project",
    "canonical_root": "/canonical/project"
  },
  "identity_context": {
    "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T11",
    "invocation": "automatic",
    "action": "context",
    "facts": {
      "session_id": "session-1",
      "thread_id": "thread-1",
      "endpoint": "unix:///absolute/socket",
      "namespace": "codex_tui"
    },
    "approval": null,
    "grant_approval": null,
    "query": false,
    "query_capability": "base64url:<32-random-bytes>"
  }
}
```

For a fact supplement, `invocation` is `supplement`. For approved identity
recovery, it is `approved_recovery` and `approval` is the exact object above.
For a separately approved grant phase, `grant_approval` is non-null and remains
a distinct owner phase. Every mutating invocation carries the capability that
the client saved before dispatch; the daemon hashes it at admission and stores
only that hash. For query, `invocation` is `query`, `query` is true,
`query_capability` is loaded from the local proof record, and `facts` may be
empty. The capability is never projected back to the caller or logged.

The daemon query request uses the same `operation_id` to select the original
operation and this exact shape:

```json
{
  "operation_id": "ctxop-01J8Z7P4V4Y8K8H6S2Q1M0R9T7",
  "invocation": "query",
  "action": "query",
  "facts": {},
  "approval": null,
  "grant_approval": null,
  "query": true,
  "query_capability": "base64url:<32-random-bytes>"
}
```

The old four-field `IdentitySupplement` remains valid only as the parser for
`--provide`. It is not the full invocation schema and cannot carry approval,
operation identity, target, generation, grant intent, or query proof.

## 7. Phase producers, reducers, replay, and query

| Phase | Producer | Durable reducer/owner | Restart replay | Query projection |
| --- | --- | --- | --- | --- |
| `admitted` | identity_gate operation admission | host-local identity operation owner | reload operation key, scope, target, intent digest, invocation | operation key, scope, target, action, outcome |
| `approval_decision` | approved-recovery admission | host-local identity operation owner | replay the approval decision and incumbent fence; never prompt or auto-approve | approved/denied decision, exact target, conflict result |
| `nested_register` | existing Register/binding owner | existing project command receipt plus outer correlation record | find nested command by persisted `command_id`; do not repeat Register | nested command id, binding id, generation, receipt state |
| `route` | existing route owner | route journal | replay route commit/rollback state; a stale or missing route is `unknown`, not success | route state, old/new binding id, generation |
| `credential` | daemon credential owner | atomic identity file and reducer credential owner | reread authoritative committed credential/cache; stale local receipt is never authority | credential present/absent/unknown; no secret in query |
| `grant` | authority owner, only when explicitly approved | authority grant ledger | replay typed grant event; identity recovery never implies replacement | current grant state and replacement receipt |
| `lease` | existing `handle_context`/lease owner | subscription/lease state | replay durable lease state; absence is not a new identity | lease state |
| `context_complete` | identity_gate result projector | no new authority; references committed snapshot owner | reconstruct from committed owner readback only | complete snapshot when safe; otherwise `partial`/`unknown` |

### 7.1 Crash windows

The outer record must be durable before the nested Register dispatch. Persist
the exact nested `command_id` and `operation_id` before calling the Register
owner. The following windows are explicit:

| Window | Durable state | Query result | Next action |
| --- | --- | --- | --- |
| before outer admission | no operation record | no queryable operation; caller may create a new key | start a new invocation |
| after `admitted`, before approval/Register | admitted + normalized intent | `approval_required` or `partial` | provide approval or facts; no replay of a side effect |
| approval persisted, before Register dispatch | approval_decision + nested ids | `partial` | query finds the durable intent; it does not repeat Register |
| Register dispatched, before receipt recorded | nested_register + nested ids | `unknown` until the project journal/receipt is readable | read receipt by nested id; never repeat Register |
| Register committed, before route/credential phase | nested_register + committed receipt | `partial` with committed phases retained | route/credential repair is a separate explicit invocation |
| route committed, before credential phase | nested_register + route | `partial` | repair separately; query remains pure |
| credential written, before lease/context complete | through credential | `partial` or `completed` only if owner readback proves it | lease/context readback or separate repair |
| project journal unavailable/incomplete | outer record intact, inner state unprovable | `unknown` | do not infer success; query again after owner availability or use separate repair invocation |

The outer owner is the only owner of the queryable `unknown` state. Existing
`CommandReceipt` remains a candidate inner receipt, but its existence alone does
not prove outer phases. `CommandStarted` without `CommandCompleted` remains
incomplete in the existing replay path; the outer projection maps that
condition to `unknown`, not to success, partial success, or automatic replay.

Cross-journal ordering is explicit:

1. outer operation record;
2. approval decision;
3. nested command start/receipt;
4. binding/route commit;
5. credential file;
6. authority grant, only if separately approved;
7. lease;
8. context snapshot.

A later phase cannot claim success from an earlier phase alone. If an earlier
phase is unknown, all dependent later phases remain `unknown` unless an
independent owner readback proves otherwise.

## 8. CLI, MCP, and daemon ownership

- CLI performs syntax validation, observes caller facts, generates or accepts
  the stable operation id and query capability, persists their local proof
  record before send, collects the exact approval object supplied by the user,
  and projects the daemon result. It does not choose an identity, mint a token,
  prove an endpoint, or decide whether an approval is valid.
- MCP calls one `collab_context` tool. It accepts `operation_id`, `provide`,
  `approve_identity`, `approve_grant`, and `query` as typed arguments; it loads
  `query_capability` from the same local proof record for query. It rejects
  unknown fields and never returns empty success for a mutation.
- Daemon `identity_gate` owns admission, approval/scope/ownership validation,
  outer phase records, command dispatch, and query projection. The resolver,
  Register transaction, route owner, credential owner, authority owner, lease
  owner, and snapshot owner keep their existing transaction boundaries.
- A caller cannot use a read key to mutate. A caller cannot use approval to
  bypass endpoint ownership. A caller cannot use a grant approval to
  authenticate an identity recovery.

## 9. Graph semantics

The context graph remains one source and one sink. Four mutually exclusive
invocations share the outer entry-to-result topology; their intent selects
which phase producers run:

```mermaid
flowchart LR
    A[读取项目与应用范围] --> B[建立或读取基线]
    B --> C[确认守护进程可用]
    C --> D[准入外层操作并验证意图]
    D --> E[处理缺事实或批准决定]
    E --> F[提交或关联内部注册]
    F --> G[提交路径与凭据]
    G --> H[按授权处理 master grant]
    H --> I[确认租约与投影上下文]
    I --> J[返回判别结果或纯查询投影]
```

The graph does not add a repair edge. `query` enters at the outer admission
node and stops at the pure projection node. A separate repair invocation is a
new graph execution with its own operation key. Failure, denial, partial,
unknown, and cancellation are named terminals selected by the result schema,
not extra parallel sinks.

## 10. Acceptance scenarios for design review

1. Automatic context with a complete canonical anchor registers or recovers
   once and returns `completed` with a snapshot and no old-token requirement.
2. Missing facts returns exact `required_fields`, no identity/binding/route/
   credential/lease/grant side effect, and a usable submit template.
3. An unapproved conflict returns `denied` or `approval_required`, never a
   guessed worker or a new identity.
4. Approved recovery with a stale local credential succeeds only when the
   daemon proves the endpoint, the exact target, and the expected incumbent;
   the old binding retires and the replacement receives the real binding id.
5. Same approval with a changed incumbent returns `APPROVAL_STALE_CONFLICT`
   and mutates nothing.
6. A changed endpoint, cross-project scope, or missing approval returns a typed
   refusal before minting a credential.
7. Same-key same-intent replay returns the original operation projection.
   Same-key different-intent returns `IDENTITY_OPERATION_INTENT_CONFLICT`.
8. Query with the retained key and locally stored capability succeeds after
   the old credential is retired. A missing or mismatched capability is denied.
9. Query after each crash window returns the phase/outcome table above,
   including `unknown`, and never repeats Register, route, credential, grant,
   or lease side effects.
10. Automatic or supplement failure after a committed phase retains
    `committed_phases` and reports `partial`/`failed`, never a false completed
    state.
11. Grant replacement occurs only when `approve_grant` is present, is distinct
    from identity recovery, and leaves authority state unchanged when it is
    absent.
12. `cancelled` is returned only at an enumerated safe boundary; it never
    erases a committed side effect.
13. A stale master identity recovers its exact binding while preserving the
    existing grant id/generation unless a separate exact `approve_grant` is
    present.
14. One invocation containing both `approve_identity` and `approve_grant`
    validates both exact approvals before admission, commits both owner phases
    at most once, and reports any partial owner result without claiming
    completion.
15. Cancellation after admission but before the first owner transaction can
    commit returns `cancelled` with no side effect; cancellation after an owner
    transaction begins is refused or reports the current `partial`/`unknown`
    state. Requery after daemon and CLI/MCP adapter restart returns the same
    durable operation without repeating an effect.

## 11. Existing text retired or reconciled

The older “exact control schema” is not the full public schema. It remains
accurate only for the four `--provide` fact fields. The following older
statements are retired:

- `IdentityContext` has no operation key and that is acceptable for a lost
  response.
- A query may repair or continue the original operation.
- Approval may be inferred from a boolean or text with no exact target and
  incumbent fence.
- The outer result may be `approved` as a success outcome.
- Automatic recovery cannot end in `partial` after registration commits.
- `TOKEN_MISMATCH` is the final behavior for the approved stale-credential
  recovery transaction.

The following statements remain authoritative:

- Automatic anchor recovery still uses the anchor ladder.
- Ambiguity, cross-project matching, and forged endpoint ownership fail
  closed.
- A user-approved replacement does not authorize cross-project takeover.
- Approval is separate from endpoint, token, and scope proof.
- Identity recovery does not implicitly promote, clear, or replace master
  authority.
- Secrets never enter snapshots, logs, messages, query projection, or Skill
  text.

## 12. Implementation gaps and review boundary

This contract requires new code for:

- an outer operation record and query projection owned by the identity gate;
- caller-retained operation ids in CLI/MCP;
- typed approval, grant-approval, and query request fields;
- a daemon-issued internal Register admission proof for approved replacement;
- pre-dispatch nested Register correlation and cross-journal phase mapping;
- safe query authorization without the old token;
- one discriminated result/transition implementation;
- graph and registry updates by the parent owner.

None of these is claimed as implemented here. A separate B23 implementation
contract must be accepted after independent design review. Until then, F2,
D3-RUC, Create, and all runtime identity writes remain unadmitted.

## 13. S23-B03 implementation delta v1 — 2026-10-09

This versioned implementation note is accepted by the parent decision in
`planner-s23-b03-replan-20261009/acceptance.md` (plan SHA-256
`c02b0691ab100bb8e87c9984c2a5b687461709149952fbea558b716c0423bc8f`). It
does not revise the public request/result schema or the semantic requirements
above.

The B23-acceptance status in the preceding section is historical and is
superseded by the accepted B23 implementation contract. B23-O is now an
accepted shared foundation; that acceptance does not itself deliver F2/D3
behavior or authorize Create/runtime identity effects outside their accepted
feature contracts and gates.

For the nested Register transaction, the outer `validating` phase must be
synced with immutable nested `command_id` and `operation_id` before any
Register consume. That phase records durable intent, not an inner receipt or
effect. The daemon prepares one actual `TypedEnvelope` from one consistent
state snapshot and consumes that same envelope; IDs are not regenerated during
consume. A sync failure prevents consume, and a failed existing CAS does not
trigger automatic reprepare, new IDs, or replay. After outer authorization, a
pure query may read the exact nested receipt from the already-loaded runtime;
it must not load/ensure a runtime, repair routes, or dispatch Register.

Existing failure, `partial`/`unknown`, legacy Register, response-loss, and
rollback semantics remain unchanged. This note is an implementation mapping
only and does not claim source behavior, S23, V23, or T3 has passed.
