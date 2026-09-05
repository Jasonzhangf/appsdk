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
project-memory index
project-memory compact
project-memory verify
```

`review` is the only task-end write-back path. It checks run notes, deduplicates,
classifies, preserves source references and tag unions, and updates the
rebuildable index. Global promotion is explicit (`--global` on an entry); a
project review never writes global memory.

The WeMM adapter is optional. Until a pinned local inference backend is
configured, it reports candidate-only status and does not mock semantic edges.
