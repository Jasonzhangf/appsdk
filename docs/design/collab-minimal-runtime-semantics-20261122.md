# Collab minimal runtime semantics

This document is the semantic baseline for the November 2022 reset. It supersedes
the older lifecycle, replay, archive, and reply-obligation designs where they
conflict with it. It is a semantic source, not runtime evidence by itself.

## 1. Authority and objects

### Object: identity map

- Identity: one identity-map row selected by canonical project and name.
- Owner: the Collab identity-map writer.
- Attributes: active current holder, agent session, transport facts, stable work
  role, and generation.
- Perception: collab context, collab board show, and the public identity tests.
- Must have: at most one active current holder for one project name.
- Must not have: a second identity source, an archive fallback, or a replayed
  historical holder.

### Object: host fact

- Identity: one fact reported by the current host, such as a Rex block, tmux
  session and pane, AppServer thread, or explicit session identifier.
- Owner: the matching host adapter.
- Perception: the host CLI or API receipt used by registration.
- Must have: a field name and a reproducible way for the host to provide it.
- Must not have: authority to select a name independently of the identity map.

### Object: name mailbox

- Identity: recipient name plus message ID inside one project.
- Owner: the mailbox state owner.
- Attributes: unread body, current recipient, and current delivery state.
- Perception: collab send, collab recv, and collab inbox.
- Must have: unread messages addressed to the stable name.
- Must not have: consumed body retention, receive replay, reply obligations, or
  a message archive.

### Object: notification subscription

- Identity: project, logical name, and subscription ID.
- Owner: the notification scheduler.
- Attributes: interval, remaining count, absolute TTL, next trigger, and current
  scheduling state.
- Perception: collab notify status, collab goal reconcile, and collab recv.
- Must have: only the current schedule and its next trigger.
- Must not have: missed-turn backlog or historical wake replay.

### Object: board task

- Identity: task ID in one project board.
- Owner: the board state owner.
- Attributes: title, description, delivery and test conditions, revision,
  owner, status, next step, dependencies, and review/integration/close facts.
- Perception: collab board show and the public board command tests.
- Must have: the minimal task fields needed to execute and close work.
- Must not have: a mandatory task execution-contract graph projection,
  next_contract, or an execution_phase audit projection.

## 2. Approved behaviors

### Behavior: register or update identity

registration facts -> one active map row with updated current holder and transport

The current host fact fills the map. Missing or conflicting facts return a
direct error naming the missing field and the supported registration entry. The
system does not guess, copy another identity, fall back to another route, or
replay history.

### Behavior: explicit takeover

human name and role instruction -> one atomic map update

Human instruction can replace the current holder of an existing name. The update
replaces the target current relation, applicable role, and target-name work in
one transaction. The replaced holder cannot reclaim the name from an old record.
The source name keeps its own work.

### Behavior: explicit retirement

explicit unregister -> released ordinary name with unchanged task ownership

An explicitly released ordinary name can be allocated to the next ordinary
registration. Existing task ownership remains unchanged until a later invite and
acceptance transfer it.

### Behavior: receive a message

unread name mailbox batch -> one recipient response with deleted message bodies

Unread messages follow the current holder of the recipient name. recv returns
one batch and removes the consumed bodies and per-message delivery bookkeeping.
Only a task result or other explicitly required business state may remain at its
own owner. Reusing a receive identifier does not replay a consumed batch.

### Behavior: restore after restart

identity map plus current durable state -> running current runtime

Daemon restart loads the current identity map, current tasks, unread mailbox,
and current subscription state. It does not rebuild history, replay retired
operations, or consume an archive.

### Behavior: schedule a periodic wake

due subscription and current holder state -> send, skip, or stay silent

A busy holder skips the turn and the next trigger advances by one original
interval. An offline holder stays silent. A finite subscription nearing exhaustion
may attach one renewal prompt to the last normally deliverable reminder. A
reminder accepted before expiry can be read once after expiry without rearming
the subscription.

### Behavior: continue a task subscription

current task owner and current subscription -> same schedule with current target

A task subscription follows current task ownership. Restart preserves its
content, remaining count, cadence, and next trigger. It does not create a second
history or reset progress.

### Behavior: architect board action

authenticated architect and entrusted task scope -> plan change or peer dispatch

An authenticated architect may create and revise plan fields for tasks in an
entrusted scope and dispatch those tasks directly to an eligible peer. Existing
revision, visibility, invitation, acceptance, status, evidence, and review gates
remain in force. An architect cannot self-grant scope, edit an unrelated task,
impersonate master authority, or bypass peer acceptance.

## 3. Relations

| Relation | Direction and cardinality | Rule |
| --- | --- | --- |
| project name -> active current holder | one-to-zero-or-one | The map is authoritative; old holders have no current claim. |
| host fact -> identity map row | many-to-one input | Facts fill or update one row; they never select a second identity source. |
| recipient name -> unread message | one-to-many | Current holder reads the unread rows for that name. |
| task -> current owner | one-to-one | Task subscriptions and board actions follow this current relation. |
| subscription -> logical name | many-to-one | Delivery targets the current holder of that name. |
| architect -> entrusted task scope | many-to-many | Explicit task creation/review or existing delegation supplies the scope. |

## 4. Retired semantics

- No reset archive: reset removes the retired control bytes and rebuilds the
  empty baseline.
- No receive replay or reply obligation: receive IDs are not message history.
- No consumed mailbox body or message archive.
- No historical identity replay, journal, or archive lookup.
- No mandatory task execution-contract DAG, next_contract, or execution_phase
  projection.
- No multi-stage handoff replay; a failed map update reports the exact current
  failure and leaves durable state unchanged.

## 5. Executable observations

The baseline binds these clauses to the public entrypoints below. Every
mandatory implementation change must keep one legal and one illegal case:

| Clause | Legal observation | Illegal observation |
| --- | --- | --- |
| identity-map only | current map row restores the name | archive/history/old route attempts to select a holder |
| missing host fact | error names the field and registration entry | guessed or copied identity |
| name mailbox | unread message is returned to the current holder | consumed body remains queryable or replayable |
| restart restoration | current map and durable state restore service | replay changes ownership or consumes a retired message |
| periodic wake | busy/offline behavior advances only the current schedule | backlog replay or a rearm after expiry |
| retirement | released name is reusable | released name remains occupied forever |
| architect authority | entrusted architect edits/dispatches within scope | self-granted architect role edits an unrelated task |
| minimal task board | board exposes task fields and revision | graph projection becomes required to execute a task |

