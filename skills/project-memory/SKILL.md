---
name: project-memory
description: "Local memory: curated L1 anchors plus fixed L2/L3 entry points with tags and relative detail paths; SQLite search over raw memory."
---

# Project memory

Use the independent `project-memory` command for memory only. It is available
at any time and is not a debug/develop gate.

`memory/index.md` is the generated local index and curation source. It includes
short titles, tags, relative detail paths, and a fixed-size Skill description
candidate section. New details live in `memory/L1/`, `memory/L2/`, or
`memory/L3/`. Raw entries remain in
`memory/{plan,path,knowledge,lesson}.jsonl`; agents may read those Markdown and
raw files directly.

The project Skill `description` is the agent-visible resident index. During
initialization or an intentional refresh, carry the generated candidate lines
into that description manually: keep deduplicated level-1 anchors first, then
fill unused base slots with level 2 and finally level 3 entries. Each L2/L3
line states its kind, tags, and relative detail path. Do not invent paths or
rewrite Skill text during an ordinary memory write. Legacy `memory/details/`
remains readable for import compatibility but is not a new output location.

## Review levels

- New CLI entries default to level 3 (`unreviewed`).
- Level 2 is reviewed and reusable; level 1 is reviewed and critical.
- Only a real review with evidence may promote an entry. Use `promote` or a
  run-note `memory_review`; ordinary writes cannot self-assign level 1/2.
- Categories (`plan`, `path`, `knowledge`, `lesson`) describe content, not
  review level. Tags are retrieval/classification features; one entry may have
  multiple tags.

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
project-memory entry --title <title> --text <content> [--tag <tag>]
project-memory entry --id <id> --category <category> --title <title> --text <content> [--tag <tag>]
project-memory query --tag <tag> [text] [project]
project-memory review --run <run-id>
project-memory promote --id <id> --level <1|2> --evidence <ref>
project-memory migrate [project]
project-memory import [project] [--global]
project-memory reentry [project] --run <run-id>
project-memory index
project-memory export
project-memory compact
project-memory verify
```

The title+content `entry` path is the one-shot writer: one invocation appends
the raw event, atomically regenerates its detail Markdown, updates the title
index, and rebuilds the SQLite projection. It defaults to level 3 and is
idempotent by generated ID. Do not write raw, detail, or index files separately
for a normal entry. If a projection step is interrupted, rerun `index`/`export`
instead of submitting the entry again.
`review` is the task-end write-back path. It checks run notes, deduplicates,
classifies, preserves source references and tag unions, and updates the
rebuildable index. A review without an evidence-backed `memory_review` remains
level 3. Global promotion is explicit (`--global` on an entry); a project
review never writes global memory.

`compact` only compacts the rebuildable projection. It never rewrites, deletes,
or drops events from the JSONL sources. The effective node is the last event
for an ID with monotonic tag/source-reference/relation unions; changing an ID
across categories is rejected.

`index`/`export` is the raw-to-Markdown compatibility bridge: it regenerates
`memory/index.md` and level-specific detail files without deleting or rewriting
JSONL. `import` is the reverse bridge for an intentionally edited exported
detail. It accepts only the marked AppSDK Markdown format (and the previous
exported detail format), validates all detail files first, appends changed
entries to the categorized JSONL source, and rebuilds the index. Repeating it
without a Markdown change is a no-op. A detail is not a second truth store:
the JSONL event history remains canonical, and direct Markdown edits
to existing IDs take effect through explicit `import`.

### Handwritten L3 additions (automatic)

Write `memory/L3/<id>.md` using this format; filename and metadata ID must match:

```markdown
<!-- project-memory:v1 {"id":"cache-key-rule","category":"knowledge","tags":["cache","debug"]} -->

# Cache keys include tenant

Include the tenant ID in cache keys to isolate tenant data.
<!-- project-memory:end -->
```

The next `query`, `get`, `index`, `export`, or `verify` imports new IDs into
JSONL and SQLite automatically, even when SQLite already exists. No daemon,
separate registration, or explicit import is needed for additions. Projection
rebuilds also ingest pending additions before exporting. Repeated access adds
no duplicate event. New entries are always L3/unreviewed; handwritten review
claims are ignored. Missing category defaults to `knowledge`.

For an existing ID, use `import` immediately after editing and before any
projection rebuild. Automatic ingestion is additions-only, not conflict
resolution between an edited detail and raw history. Invalid files fail with
an actionable error and remain on disk; repair their format, do not delete
memory to make a query pass. New L1/L2 files are not automatically ingested.

Compatibility paths:

```text
legacy entries.jsonl/memories.jsonl --migrate--> categorized JSONL
categorized JSONL --index|export--> index.md + L1/L2/L3/<id>.md
edited marked detail --import--> categorized JSONL event + rebuilt projection
```

Malformed, unmarked, duplicate-ID, or conflicting details fail explicitly;
they are never guessed into memory. `import --global` applies the same rule to
the global detail directory.

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

Re-entry may ingest new L3 details when it reads memory and rebuild stale or
missing SQLite projections. Run
`project-memory reentry [project] --run <run-id>` after an interruption (the
project path is optional and may also follow the `--run` value). It keeps the
same run ID, reads the last note as `resume_from`, checks the migration marker,
rebuilds a missing index, and returns L1 anchors plus bounded `next_queries`
for the next L2/L3 lookup. If migration is absent or unfinished, re-entry is
blocked with the exact migration command; it never invents a new run or state.

The WeMM adapter is optional. Until a pinned local inference backend is
configured, it reports candidate-only RAG/semantic candidates and does not mock
semantic edges. Graph search uses declared and semantic relation edges returned
by `query`. Open the returned `detail_path` directly when the title is relevant;
do not copy full detail into the index.
