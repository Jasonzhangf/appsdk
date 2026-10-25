# AppSDK Development Process Control Harness — Test Design

## Contract compile

- same inputs in two roots produce byte-identical compiled manifests;
- only project-declared sources are read;
- missing optional AGENTS is reported, not blocked;
- missing required Skill or machine contract fails with one next action;
- absolute, parent traversal, and symlink rule paths fail;
- duplicate source, workflow, node, edge, or rule IDs fail;
- advisory, warning, and forbidden severities validate.
- bundled rules make changed-scope architecture violations forbidden while
  untouched historical architecture debt remains advisory;

## Compatibility

- existing `new -> verify` remains green without compiling guidance;
- an already governed root can rerun idempotent `init` without creating a new
  preparation record;
- removing `guidance` from a project leaves `verify` and `compile` behavior
  unchanged;
- no PlanRecord is required by ordinary `verify` or `compile`.

## Status and routing

- status projects project/module lifecycle without creating files;
- a missing module source path is projected as a module-binding failure with
  module owner and contract repair action, and recovers after a real rebind;
- every supported domain resolves through one engine;
- an unknown domain fails without writing state;
- a project without a Guidance declaration returns `GUIDANCE_SETUP_REQUIRED`
  and the bootstrap intake command;
- a configured project with a missing compiled manifest returns
  `GUIDANCE_NOT_COMPILED` and the compile action without retrying automatically.

## Initialization intake

- `new` prints the initial `guide compile -> guide init` onboarding route;
- configured repeated `init` refreshes the versioned standard template and
  prints the read-only `guidance-upgrade` bootstrap route;
- an existing governance-only project preserves its project contract and
  records, and `init` prints the bootstrap setup intake route;
- `guide --help` exposes every command, including `guide init`;
- uncompiled intake returns the missing compile/init commands without writing;
- bootstrap intake works before and after compile, returns current root AGENTS,
  the advisory standard template path and version, bundled AppSDK Skill, and
  direct project-local Skill candidates, and never scans nested or unrelated
  directories;
- bootstrap intake returns project/module state, only declared unresolved
  questions, a `GuidanceSetupProposal` schema, and post-approval compile/verify
  commands;
- bootstrap intake emits no fixed internal `required` questions such as
  `standard_template_comparison`, `standard_workflows`, `project_commands`,
  `rule_ownership`, or `approval_boundary`, and adds no mandatory human
  confirmation chain;
- a genuinely uncovered durable authorization still carries an accurate
  conditional instruction naming the precise missing decision and affected
  scope;
- bootstrap intake does not modify project.json, lifecycle records, Skills,
  compiled guidance, or `.appsdk-control`;
- template upgrade intake requires user approval only for uncovered durable
  changes, distinguishes recommended, retained, and declined items, never
  overwrites project rules, and never declares the standard template as a rule
  source;
- removing the advisory template does not fail ordinary `appsdk verify`, while
  a symlinked reference path fails safely without modifying its target;
- compiled intake returns declared AGENTS/Skill sources, exact Skill commands,
  project/module/goal context, and the next guide commands;
- develop and debug intake return only declared unresolved goal questions;
  internal questions route to the owning Skill or Collab peer, and the user is
  asked only for user-exclusive facts, new authorization, or hard-constraint
  changes;
- intake never creates `.appsdk-control` task state; `guide plan` remains the
  first write.
- task PlanProposal never mutates or replaces the project-level setup proposal
  or durable project rules.

## Plan

- a valid adjacent agent plan is persisted with derived context hashes;
- agent-supplied `current_node` or `next_transition` is rejected;
- non-adjacent nodes, unknown owner, out-of-scope path, and empty plan fail;
- a second plan requires a valid revision reason and preserves prior history;
- changed scope/owner/rules/source cannot silently reuse the old plan.

## Update and next

- passing a required step without evidence fails;
- only the projected next step may be updated;
- same event ID/content is an idempotent no-op;
- same event ID/different content fails;
- source, tree, project, goal, Skill, AGENTS, or manifest drift blocks update;
- pass advances one adjacent node; fail/blocked does not skip;
- close reports workflow and AppSDK lifecycle completion separately;
- close emits memory candidates but never edits durable memory/rule files.
- feature/debug/review nodes require changed-scope architecture-conformance
  evidence and describe historical findings as non-blocking recommendations.
- tour persists a user-selected adjacent path without changing the active plan;
  review rejects flow changes until every selected node has an accepted content
  revision, then stages a flow patch carrying those node revision IDs.
- rejected node content keeps flow review blocked until a later node review
  supersedes it with an accepted revision; active compiled guidance is never
  mutated implicitly.
- review-node memory reminders are advisory and do not appear as debug/develop
  gates; `project-memory review` remains an independent task-end write path.
