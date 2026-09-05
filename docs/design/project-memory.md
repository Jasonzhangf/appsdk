# Project memory

## Boundary

`project-memory` is independent from `guide`, `debug`, and `develop`. It owns
memory classification, query, compaction, and reviewed write-back. Governance
still owns lifecycle/evidence; Collaboration owns workers and worktrees; Guide
owns process nodes and edges.

## Durable source and projection

```text
memory/index.md                         L1 stable anchors
memory/{plan,path,knowledge,lesson}.jsonl  L3 durable entries
        |
project-memory index                   rebuildable L2 projection
        |-- global/index.sqlite
        |-- projects/<project-id>/index.sqlite
        |-- FTS5 keyword table
        `-- vec_entries semantic adapter table
```

The source files remain recoverable and human-readable. SQLite may be deleted
and rebuilt. `vec_entries` records the pinned WeMM adapter contract; until a
local WeMM server is configured, semantic results are candidate-only and never
change explicit relations.

## Categories and retrieval

Every entry has one category: `plan`, `path`, `knowledge`, or `lesson`. Query
groups are stable: L1 anchors, exact ID, node/function/resource, category/tag,
declared relations, lesson references, FTS5, WeMM candidates, then importance
and recency. The response includes `next_queries` so the agent can expand into
the next layer without changing the resident L0 protocol.

## Reviewed write-back

`project-memory review --run <run-id>` checks completed run notes. Only an
explicit `memory`/`memory_update` candidate is classified and appended; absent
candidates return `no_update`. Tags and source references are unioned during
`compact`; raw entries remain recoverable. Review never promotes project data to
global memory. Global writes require an explicit `entry --global` action.

## Guide hand-off

Guide close exposes a non-blocking `project-memory review` reminder. The Guide
tour/review state machine remains independent: it first accepts node content,
then accepts flow edges/order/rules, and stores accepted flow patches with node
revision IDs. Memory semantic associations are advisory and cannot reorder or
activate a process.
