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
- removing `guidance` from a project leaves `verify` and `compile` behavior
  unchanged;
- no PlanRecord is required by ordinary `verify` or `compile`.

## Status and routing

- status projects project/module lifecycle without creating files;
- a missing module source path is projected as a module-binding failure with
  module owner and contract repair action, and recovers after a real rebind;
- every supported domain resolves through one engine;
- an unknown domain fails without writing state;
- missing compiled manifest returns `GUIDANCE_NOT_COMPILED` and the compile
  action without retrying automatically.

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
