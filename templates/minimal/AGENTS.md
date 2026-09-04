# Project Agent Contract

This file owns project facts and boundaries. Replace bracketed items during the
approved Guide setup; do not invent project-specific values. Reusable procedure
belongs in project Skills, and machine workflow belongs in declared guidance
contracts.

## Project Truth

- Purpose and user-visible contract: [describe].
- Active implementation and production entrypoints: [describe].
- Compatibility and legacy boundaries: [describe].

## Semantic Invariants

- Preserve the request, response, state, and error semantics declared by this
  project.
- Fail explicitly at the owning boundary; do not add guessed repair, fallback,
  downgrade, silent drop, or success-wrapped error.
- Keep control state separate from business payload and metadata when the
  project has both planes.
- One semantic responsibility has one owner and one implementation.
- Control and configuration truth uses declared typed control resources, error
  chains, or project configuration sources. Request/response payloads,
  metadata, debug logs, and implicit context never carry control truth.

## Ownership

- Declare each module, resource, mutable truth, and cross-module edge with one
  owner.
- Record allowed and forbidden paths at the narrowest stable boundary.
- Derived output never becomes a second source of truth.
- Keep lifecycle skeletons fixed. Configure operations, register hooks, and
  declare gates through their owning contracts instead of adding inline
  behavioral branches.
- Before adding behavior, prove it is necessary and not already owned. Reuse a
  shared function for common semantics; retain separate implementations only
  for necessary differences.
- Missing operators, hooks, or gates fail or skip explicitly with a recorded
  reason. They never produce mock success.

## Architecture Truth

- Declare the project resource, function, mainline, verification, and module
  maps and keep them aligned with source anchors and gates.
- Missing or ambiguous ownership blocks the affected change, not unrelated
  project work.
- Update maps and verification gates in the same change when ownership, paths,
  call edges, or regression coverage changes.

## Development Process Control

- Use AppSDK Guide for non-trivial feature, debug, review, delivery, and cleanup
  work.
- Persist plans bound to the current goal, task, module, owner, scope, declared
  rule sources, source commit, and tree.
- Execute only declared transitions. Optional nodes are skipped through an
  explicit bypass edge, never an undeclared jump.
- Append observations and evidence to the active step. Revise the plan when
  source, scope, owner, rules, environment, or evidence changes.
- Workflow close and lifecycle completion are separate results.
- Review new and changed behavior as a hard boundary for control-truth
  separation, single ownership, configuration-first orchestration, registered
  hooks, declared gates, ablation, and shared-function reuse. Report untouched
  historical violations as non-blocking recommendations unless they affect the
  changed scope or a safety/evidence boundary.

## Git Protection

- Treat the protected mainline checkout as read-only. Develop in a clean owner
  worktree created from the latest remote mainline and preserve other workers'
  state.
- Configure the project's own commit and push protection. A passing protection
  check proves only the Git boundary.
- Remove the owned worktree and release its claim only after required delivery,
  remote receipt, and retention evidence exist.

## Task Routing

Declare project-owned routes during Guide setup:

| Need | Project-owned source |
| --- | --- |
| requirements and architecture | [document or Skill] |
| feature development and debug | [Skill] |
| build, install, restart, and replay | [document or adapter] |
| review and delivery | [Skill or contract] |

## Evidence Boundary

- Report source, test, build, installed artifact, restart, deployed-entrypoint
  replay, review, merge, remote receipt, freeze, and cleanup separately.
- Never infer a later evidence level from an earlier one.
- A blocked result names the first failing gate, preserved state, retry policy,
  owner, and one executable next action.
