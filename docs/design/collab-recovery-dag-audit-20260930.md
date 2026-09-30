# Collab identity recovery DAG and independent audit plan (2026-09-30)

## Decision intent

- daemon dead or unreachable is a lifecycle failure, not identity recovery;
- daemon alive is required for live/dead conflict classification before recovery;
- normal session/thread/pane drift auto-recovers by rebinding the durable identity to current facts;
- a live conflict fails closed by default;
- explicit user override is the only path that may retire or supersede a live conflict and then recover.

## Single source

Recovery input is only:

1. persisted identities: project scope plus historical anchors;
2. current endpoint facts: project scope, session id, thread id, pane/endpoint;
3. daemon liveness classification: live, dead, or unknown.

Cross-project reuse, route-journal guessing, and generation equality are not compatibility criteria.

## Single sink

Each recovery request reaches exactly one terminal state:

- DAEMON_UNAVAILABLE: do not attempt recovery;
- AUTO_RECOVERED: compatible durable identity, no live conflict, rebound to current facts, fresh binding generation;
- DENIED_LIVE_CONFLICT: live compatible identity exists, no explicit override, report exact conflict identities;
- OVERRIDE_RECOVERED: explicit override retires or supersedes the live conflict, then rebinds durable identity and publishes a fresh generation;
- NO_COMPATIBLE_IDENTITY: fresh registration path only.

## DAG

current endpoint facts
  -> daemon reachability
     |-- unavailable -> DAEMON_UNAVAILABLE (lifecycle failure; no identity recovery)
     |-- alive -> collect compatible persisted identities by project scope
                  |-- empty -> NO_COMPATIBLE_IDENTITY (fresh registration)
                  |-- non-empty -> classify each same-scope peer liveness
                                   |-- provably dead -> archive (never blocks)
                                   |-- live and no override -> DENIED_LIVE_CONFLICT
                                   |-- live and override -> retire/supersede conflict
                                   |-- no live and exactly one unknown -> adopt that durable identity
                                   |-- no live and multiple unknown -> DENIED_LIVE_CONFLICT (ambiguous; override required)

selected durable candidate
  -> rebind to current endpoint facts
  -> publish fresh binding generation and host index
  -> AUTO_RECOVERED or OVERRIDE_RECOVERED

## Compatibility

Compatible means same project scope plus trustworthy anchor overlap or an explicitly named worker id. Exact current session/thread/pane equality is not required.

## Liveness

- live: transport, thread, or pane probe confirms active;
- dead: explicit dead, retired, or notLoaded signal;
- unknown: probe is inconclusive or there is no transport.

Unknown does not count as a live conflict. A single unknown same-scope durable
candidate is adopted (normal drift auto-recovery). Multiple unknown candidates
are ambiguous and require an explicit worker override. An unknown candidate is
never silently used to authorize an override.

## Independent audit checks

1. identity recovery does not require exact current session/thread/pane when a unique same-scope durable candidate exists or a single unknown same-scope candidate exists;
2. recovery has one source set and one terminal sink set;
3. live conflict is checked before approval;
4. dead or unknown candidates cannot block recovery when they do not create a live conflict and selection is deterministic;
5. explicit user override is the only way to supersede a live conflicting identity;
6. daemon-unavailable is a lifecycle failure and is not retried as identity recovery;
7. generation is a binding publication fence, not a recovery compatibility criterion.

## Required tests

1. session, thread, or pane changed: auto recovery rebinds current facts;
2. pane disappears or restarts: recovery uses remaining durable anchors;
3. live compatible candidate without override: denied and names conflict identity;
4. live compatible candidate with override: recovered after retiring or superseding conflict;
5. dead candidates do not block recovery;
6. single unknown same-scope candidate is adopted; multiple unknown candidates require override;
7. no compatible identity uses the fresh registration path;
8. daemon unreachable publishes no identity recovery state.

## Audit note

This document is the single-source recovery DAG for the 2026-09-30 candidate.
Independent recovery-DAG audit was attempted; the current document reflects the
user-corrected policy. Final independent architecture review is still required
before merge and is not claimed here.

## Delivery gates

- targeted identity and route tests in candidate worktree codex/collab-daemon-lifecycle-audit-20260929;
- full Collab tests;
- independent architecture review PASS;
- release build, install, and installed digest match candidate;
- controlled collab up/down, context, route resolve, MCP initialize, and live send-recv;
- merge and push to origin/main;
- cleanup only this task's worktree, playground, and temporary resources.
