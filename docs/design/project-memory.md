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
under the scope lock, then regenerates the detail, title index, and rebuildable
projection. The atomic commit boundary is one classification JSONL file
replacement. The Markdown and SQLite projections are separate outputs. A
projection failure after the raw commit is reported as committed, and the
projection can be rebuilt without submitting the entry again. SQLite may be
deleted and rebuilt from raw events. `vec_entries` records the pinned WeMM
adapter contract; until a local WeMM server is configured, RAG/semantic results
are candidate-only and never change explicit relations.
The SQLite `index_profile.schema_version` is written from the single
`MEMORY_SCHEMA_VERSION` constant. `project-memory verify` reports the stored
value and treats a missing or mismatched value as an unhealthy stale index.

## Review levels and retrieval

Every entry has one category: `plan`, `path`, `knowledge`, or `lesson`; this is
separate from its review level. New CLI entries default to level 3 and may be
searched before review. Level 2 is reviewed/reusable; level 1 is reviewed and
critical. Only evidence-backed review can promote level. Tags are retrieval
features and may be searched with `query --tag`.

Review level and status describe retrieval and review state. They do not approve
a rule. A review pass, task acceptance, or non-empty `review_evidence` cannot
replace the explicit user approval required for rule promotion.

The agent-visible resident index is the project Skill frontmatter description,
not the entire Markdown index. Curation uses a fixed base line budget: keep
deduplicated level-1 anchors first, then fill remaining lines with level-2 and
level-3 entries. The CLI generates each compact L2/L3 line with its kind, tags,
and relative detail path; the agent performs the one-time architecture
deduplication and carries the selected lines into the Skill description.
Ordinary Memory writes never mutate Skill text automatically.

Query groups are stable: level-1 titles, exact ID, category/tag, declared graph
edges, lesson references, FTS5, RAG candidates, then importance and recency.
Results return `detail_path`; an agent may open that Markdown file directly.

## Reviewed write-back

`project-memory review --run <run-id>` checks completed run notes. Only an
explicit `memory`/`memory_update` candidate is classified and appended; absent
candidates return `no_update`. Without a real evidence-backed
`memory_review`, the candidate stays at level 3. `project-memory promote` is
the explicit promotion path and requires level 1/2 plus evidence. For a rule,
promotion also requires a complete clause and a matching user approval
declaration. `compact` updates only the rebuildable index: the latest event owns
content while tags, source references, and relations are unioned; raw JSONL
events remain unchanged and recoverable. `rule_control` is not unioned or
inherited from an older event. Review never promotes project data to global
memory. Global writes require an explicit `--global` action.

## Rule promotion

`entry_kind` separates `fact` and `rule`. A missing value means an old or
unclassified record; it does not become a fact automatically. Facts and rule
candidates can be saved and queried without approval. A rule candidate may have
an incomplete clause, but it is not an approved or executable rule.

The latest raw event owns the complete effective `rule_control`. Its clause
contains the rule's scope, scenario, trigger, allowed and forbidden actions,
state change, effects, owner, evidence, and target governance binding. A rule
revision covers the title, content, and complete clause. Tags, source
references, importance, review fields, timestamps, projections, and approval
metadata do not change the revision.

Approval is separate from review. The approval declaration names the user role,
the target Memory scope, entry ID, and exact rule revision, plus its source and
original approval text. The target scope identifies the project or global
Memory store. It does not replace `rule_control.clause.scope`, which states the
business scope of the stored rule. A mismatched scope, ID, or revision is
rejected before raw commit. The machine validates the declaration; it does not
authenticate a person.

The structured submission and promotion paths are:

```text
project-memory entry <project> --input <entry.json> [--global]
project-memory promote <project> --id <id> --level <1|2> \
  --evidence <review-ref> --approval <approval.json> [--global]
project-memory import [project] [--global]
```

`--input` is the structured entry channel. A new rule is always saved as a
candidate, even when its input claims approval. The result must state that the
approval was not applied because a new entry needs a later promotion. For an
existing candidate, `promote --approval` submits the matching approval and the
requested review level in one interaction. Explicit import can carry the same
structured data for an existing candidate. Run-note review, automatic L3
intake, and migration can save facts, candidates, or review evidence; they
cannot submit approval or promote a rule to level 1/2.

`--global` selects the global Memory scope. It does not change a project rule
into a globally approved rule, and the approval declaration must match that
global scope. Without `--global`, the command uses the project scope.

Promotion runs through the Memory operation bound to the existing SDK
`bind_operation` commit boundary. The lock-protected commit rereads current raw
state, validates the complete clause and approval target, replaces one
classification JSONL file, and returns a typed raw receipt. Markdown and SQLite
projection updates happen after that raw commit and are not part of its atomic
boundary.

If projection fails after commit, the JSON result reports
`accepted=true`, `committed=true`, and `projection_updated=false`, with the real
error and a recovery command. The recovery command is:

```text
project-memory index <project> --rebuild-only
project-memory export <project> --rebuild-only
project-memory index <project> --global --rebuild-only
project-memory export <project> --global --rebuild-only
```

`--rebuild-only` reads raw state and rebuilds the selected projection. It does
not scan Markdown input, accept new L3 entries, append raw events, or repeat
promotion. A successful Memory promotion proves only that Memory accepted and
stored the rule revision. It does not prove that another consumer or downstream
gate has executed the stored rule.

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
