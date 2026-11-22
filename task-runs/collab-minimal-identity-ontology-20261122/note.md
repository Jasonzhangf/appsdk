# Collab minimal identity and mailbox ontology baseline

## Long-term goal

Merge the user-approved November runtime semantics into current origin/main,
remove the conflicting archive/replay/reply layers, and deliver one clean main
baseline.

## 2026-11-22 approval and scope

The user selected the clean November baseline and approved eight semantic
groups: simplified periodic wake, task subscription continuity, explicit
retirement with name reuse, atomic takeover, unread-only mailbox retention,
architect board authority, no mandatory task execution DAG, and host facts as
identity-map input only. The user separately required no history replay and no
archives.

The semantic source is docs/design/collab-minimal-runtime-semantics-20261122.md.
The stale-name merge already preserves released generated-name reuse and ping
timeout handling.

## Current step

Apply the authoritative requirement replacements, add executable board and
mailbox regressions, ablate reset archives and receive replay, validate the
affected DAGs and tests, then integrate to main.

## Requirement receipts

- 2026-11-22: req-collab-identity replaced 5 -> 6; ledger version 23.
- 2026-11-22: req-collab-communication-notifications replaced 8 -> 9; ledger
  version 24.
- 2026-11-22: req-collab-architect-board-authority created at version 1;
  ledger version 25.

All three changes were submitted through appsdk requirements apply; the ledger
was not edited manually.
