# Project memory

## Boundary

`project-memory` is independent from `guide`, `debug`, and `develop`. It owns
memory classification, query, compaction, and reviewed write-back. Governance
still owns lifecycle/evidence; Collaboration owns workers and worktrees; Guide
owns process nodes and edges.

## Durable source and projection

```text
memory/index.md                         Markdown index and Skill curation source
memory/L1|L2|L3/<id>.md                  human-readable level detail
memory/{plan,path,knowledge,lesson}.jsonl  append-only raw events
        |
project-memory index                   rebuildable L2 projection
        |-- global/index.sqlite
        |-- projects/<project-id>/index.sqlite
        |-- FTS5 keyword table
        `-- vec_entries semantic adapter table
```

The JSONL events and Markdown details remain readable and recoverable. The
index contains short headings, tags, relative detail links, and a fixed-size
Skill description candidate section; it does not copy detail content.
Markdown details use a small JSON metadata marker so they can be imported back
after an intentional edit. JSONL remains the canonical event history; Markdown
is a human-readable interchange view, not a second mutable truth store.
Normal writes are one-shot: a single `entry` invocation appends the raw event
and atomically regenerates the detail, title index, and rebuildable projection.
SQLite may be deleted and rebuilt from raw events. `vec_entries` records the
pinned WeMM adapter contract; until a local WeMM server is configured, RAG/
semantic results are candidate-only and never change explicit relations.

## Review levels and retrieval

Every entry has one category: `plan`, `path`, `knowledge`, or `lesson`; this is
separate from its review level. New CLI entries default to level 3 and may be
searched before review. Level 2 is reviewed/reusable; level 1 is reviewed and
critical. Only evidence-backed review can promote level. Tags are retrieval
features and may be searched with `query --tag`.

The agent-visible resident index is the project Skill frontmatter description,
not the entire Markdown index. Curation uses a fixed base line budget: keep
deduplicated level-1 anchors first, then fill remaining lines with level-2 and
level-3 entries. The CLI generates each compact L2/L3 line with its kind, tags,
and relative detail path; the agent performs the one-time architecture
deduplication and carries the selected lines into the Skill description.
Ordinary Memory writes never mutate Skill text automatically.

Query groups are stable: level-1 titles, exact ID, node/function/resource,
category/tag, declared graph edges, lesson references, FTS5, RAG candidates,
then importance and recency. Results return `detail_path`; an agent may open
that Markdown file directly.

## Reviewed write-back

`project-memory review --run <run-id>` checks completed run notes. Only an
explicit `memory`/`memory_update` candidate is classified and appended; absent
candidates return `no_update`. Without a real evidence-backed
`memory_review`, the candidate stays at level 3. `project-memory promote` is
the explicit user/audit promotion path and requires level 1/2 plus evidence.
`compact` updates only the rebuildable index: the latest event owns content
while tags, source references, and relations are unioned; raw JSONL events
remain unchanged and recoverable. Review never promotes project data to global
memory. Global writes require an explicit `entry --global` action.

## Migration and re-entry

The memory path has two explicit recovery routes:

```text
legacy flat JSONL
  -> validate all source lines
  -> memory/migration.json (in_progress)
  -> append new or metadata-completing categorized events
  -> compact + rebuild SQLite
  -> memory/migration.json (complete)
```

`project-memory migrate` keeps the legacy source, records its digest and
progress, and is safe to repeat after interruption. It refuses a changed
source or conflicting ID/content rather than replacing project truth. When no
legacy source is present it records an idempotent `not_needed` migration.

`project-memory index` and its alias `project-memory export` are the raw-to-
Markdown compatibility bridge for old and current records. They emit the same
Markdown index/detail view without deleting or rewriting the JSONL input, so
older memory remains readable. `project-memory import` is the explicit reverse
bridge: it reads marked exported details (and the previous exported detail
format), validates the complete directory before writing, appends only changed
entries to the categorized JSONL source, and rebuilds the SQLite/Markdown
projection. Repeating import is idempotent. Invalid or ambiguous Markdown is
reported, not silently converted.

New marked `memory/L3/<id>.md` files are ingested automatically on query/get,
index/export, or verify, before projection generation. The same parser and
event writer are reused; the whole additions batch is validated first, then
appended without recursive projection rebuilds. Filename must match the ID.
Only IDs absent from raw history are admitted automatically, always as
unreviewed L3 with no supplied review evidence. Existing IDs still require
explicit import for edits. Repeated access is idempotent. SQLite absence or
staleness does not change these rules. Global memory uses the same scope-local
behavior. See the project-memory Skill for the minimal handwritten format.

```text
legacy flat JSONL -> migrate -> categorized JSONL
categorized JSONL -> index/export -> Markdown index + L1/L2/L3 details
edited Markdown details -> import -> categorized JSONL events -> rebuilt projection
```

If a projection update is interrupted after the raw event is committed, rerun
`index` or `export`; do not submit the same `entry` again. The source event is
append-only and the projection is recoverable.

```text
same run ID
  -> read notes.jsonl last state
  -> check migration marker
  -> rebuild missing index
  -> return L1 anchors + next_queries
  -> query L2/L3 and continue the interrupted node
```

`project-memory reentry [project] --run <run-id>` never creates a new run or
mutates durable memory. The project path is optional and may follow the run ID.
It blocks with the migration command when migration is absent
or unfinished, and returns the same run ID plus the last known node/step for
resume.

For existing IDs the Markdown bridge is explicit: `project-memory export` renders
the current raw history, while `project-memory import` reads an edited detail
back into raw history. The title index alone is not importable because it does
not contain the detail body. This keeps the directory convenient for humans
without creating competing sources of truth.

## Guide hand-off

Guide close exposes a non-blocking `project-memory review` reminder. The Guide
tour/review state machine remains independent: it first accepts node content,
then accepts flow edges/order/rules, and stores accepted flow patches with node
revision IDs. Memory semantic associations are advisory and cannot reorder or
activate a process.
