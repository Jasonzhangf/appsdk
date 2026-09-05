# Project memory index

This is the stable L1 entrypoint for local project memory. Keep this page short,
project-neutral, and rarely changing so resident context and cache hits remain
stable. Ordinary memory writes update lower layers only.

## Entrances

- [Plan](plan.jsonl): project goals, architecture, owners, boundaries, and constraints.
- [Path](path.jsonl): nodes, edges, flow paths, checklists, and execution rules.
- [Knowledge](knowledge.jsonl): one discrete verified fact, contract, function, resource, or setting.
- [Lesson](lesson.jsonl): verified historical experience, root cause, pitfall, or resolution.

## Scope

Ordinary entries belong in one of the four JSONL files. Do not rewrite this page
for an individual entry: it is the stable L1 map, while SQLite and FTS are
rebuildable projections. Use the project-memory skill for ordered retrieval,
reviewed write-back, and WeMM candidate handling.
