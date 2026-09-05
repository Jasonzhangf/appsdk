---
name: project-memory
description: Local, layered project memory with deterministic query, compaction, and reviewed write-back.
---

# Project memory

Use the independent `project-memory` command for memory only. It is available
at any time and is not a debug/develop gate.

## Layers and categories

- L0: this stable retrieval protocol; keep it short and rarely change it.
- L1: `memory/index.md` anchors and four entrances.
- L2: rebuildable SQLite/FTS index and explicit relations.
- L3: durable `memory/{plan,path,knowledge,lesson}.jsonl` entries.

Classify every durable entry as exactly one of:

- `plan`: project goal, architecture, owner, boundaries, constraints.
- `path`: node, edge, flow, checklist, or execution rule.
- `knowledge`: one discrete fact, contract, function, resource, or setting.
- `lesson`: verified historical experience, root cause, pitfall, or resolution.

## Fixed query order

Keep result groups in this order: L1 anchors, exact ID, node/function/resource,
category/tag, declared relations, lesson references, FTS5 keywords, WeMM
semantic candidates, then importance and updated time. Semantic candidates are
advisory and never change explicit flow edges or active process revisions.

## Commands

```text
project-memory query <text> [project]
project-memory get <memory-id> [project]
project-memory entry --id <id> --category <category> --text <text> [--tag <tag>]
project-memory review --run <run-id>
project-memory migrate [project]
project-memory reentry [project] --run <run-id>
project-memory index
project-memory compact
project-memory verify
```

`review` is the only task-end write-back path. It checks run notes, deduplicates,
classifies, preserves source references and tag unions, and updates the
rebuildable index. Global promotion is explicit (`--global` on an entry); a
project review never writes global memory.

`compact` only compacts the rebuildable projection. It never rewrites, deletes,
or drops events from the JSONL sources. The effective node is the last event
for an ID with monotonic tag/source-reference/relation unions; changing an ID
across categories is rejected.

## Migration and re-entry

Migration is explicit and source-preserving. `project-memory migrate` accepts
the supported legacy flat sources (`memory/entries.jsonl` or
`memory/memories.jsonl`), validates every line before writing, records a
versioned `memory/migration.json` marker, appends new entries or
metadata-completing events when the effective version lacks incoming tags or
source references, and rebuilds the SQLite projection. The legacy source is
never deleted or overwritten. A marker in `in_progress` can be run again with
the same source digest; completed migration is idempotent. A changed source or
an ID/content conflict is reported instead of silently replacing project
truth.

Re-entry is read-only apart from rebuilding a missing SQLite projection. Run
`project-memory reentry [project] --run <run-id>` after an interruption (the
project path is optional and may also follow the `--run` value). It keeps the
same run ID, reads the last note as `resume_from`, checks the migration marker,
rebuilds a missing index, and returns L1 anchors plus bounded `next_queries`
for the next L2/L3 lookup. If migration is absent or unfinished, re-entry is
blocked with the exact migration command; it never invents a new run or state.

The WeMM adapter is optional. Until a pinned local inference backend is
configured, it reports candidate-only status and does not mock semantic edges.
