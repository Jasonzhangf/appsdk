# AppSDK Development Process Control Harness — Test Design

## Contract compile

- same inputs in two roots produce byte-identical compiled manifests;
- only project-declared sources are read;
- missing optional AGENTS is reported, not blocked;
- missing required Skill or machine contract fails with one next action;
- absolute, parent traversal, and symlink rule paths fail;
- duplicate source, workflow, node, edge, or rule IDs fail;
- advisory, warning, and forbidden severities validate.

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

- `new` and configured `init` print the `guide compile -> guide init`
  onboarding route;
- an existing governance-only project preserves its project contract and
  records, and `init` prints the bootstrap setup intake route;
- `guide --help` exposes every command, including `guide init`;
- uncompiled intake returns the missing compile/init commands without writing;
- bootstrap intake works before compile, returns root AGENTS, bundled AppSDK
  Skill, and direct project-local Skill candidates, and never scans nested or
  unrelated directories;
- bootstrap intake returns project/module state, user questions, a
  `GuidanceSetupProposal` schema, and post-approval compile/verify commands;
- bootstrap intake does not modify project.json, lifecycle records, Skills,
  compiled guidance, or `.appsdk-control`;
- compiled intake returns declared AGENTS/Skill sources, exact Skill commands,
  project/module/goal context, and the next guide commands;
- develop intake asks requirements, architecture, and delivery questions;
- debug intake asks failing-sample, causal-evidence, and replay questions;
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
