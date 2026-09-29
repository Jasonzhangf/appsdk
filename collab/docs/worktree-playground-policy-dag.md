# Configured Worktree Playground Policy DAG

Status: review candidate, not implementation.

## Objective

Move new project worktree creation out of project main trees into a configured external base, while preserving existing task records and migration inputs that still use the legacy `<project-main>/playground/<task-slug>` shape.

Required behavior:

1. New worktrees are created only under the configured external playground base.
2. Project isolation is explicit and per-project.
3. Existing legacy records continue to pass migration and lifecycle checks during the transition.
4. Path policy is owned by config, not by code-literal disk locations.
5. Collab validation, CLI docs, embedded skills, tests, runtime delivery, and cleanup all close through one DAG.

## Inputs and Owners

- User-approved policy: external playground base, per-project isolation, legacy records continue.
- Runtime config owner: `collab/src/config.rs`.
- Server route owner: canonical project root from Collab route/scope, not cwd alone.
- Worktree validation owner: one policy resolver in Collab server code.
- Skill/source text owner: repository skill sources under `collab/skills/collab/`, installed by `collab install-skills`.
- Migration owner: legacy records remain valid; new configured paths are accepted without changing already-imported records.

## Configuration Contract

Add a typed config section in `collab/src/config.rs`:

```toml
[worktree]
base = "/absolute/path/to/playground"
layout = "{project-key}/{task-slug}"
```

Validation:

- `base` is required for new configured worktree creation when this feature is enabled.
- `base` must be absolute.
- `base` must canonicalize at validation time; an unavailable or unresolvable base fails closed.
- `layout` must have exactly two path segments.
- `layout` must contain the placeholders `{project-key}` and `{task-slug}` once each.
- Neither placeholder may appear in `base`.
- `{project-key}` and `{task-slug}` render to sanitized ASCII slugs.
- Final path must remain under canonical `base` after replacement.
- The final directory is project-isolated by rendered `{project-key}`.

The default must not create a local fallback path. If no configured `worktree` section exists, configured worktree creation is unavailable and the caller receives a typed error naming the missing config. Existing legacy validation remains separate from configured creation.

Project key ownership:

- Prefer an explicit config-derived project key if a future config schema provides it.
- For this change, do not invent a global project registry. Use a deterministic sanitized key derived from the canonical project root basename when basename uniqueness cannot be proven.
- This is acceptable because this DAG only requires per-project separation, not globally unique human labels. If collisions are introduced later, make the project key an explicit config field before relying on uniqueness.

## Compatibility Contract

Legacy accepted during transition:

- `<project-root>/playground/<task-slug>`
- relative spellings equivalent to `./playground/<task-slug>` and `playground/<task-slug>`
- existing migration records with legacy relative worktree paths

New configured accepted:

- `<rendered-worktree-base>/<task-slug>` where rendered path is under configured base and contains the project key segment.

Rejected:

- new worktree paths outside the configured base after configuration exists.
- configured paths escaping `base` through `..`, symlink, empty segments, or placeholder abuse.
- ambiguous relative paths that could resolve to both legacy and configured interpretations.
- project main tree worktree creation for new work when configured base exists.

Migration rule:

- Do not rewrite existing legacy records automatically.
- Preserve original stored `worktree_path` bytes unless a documented future migration explicitly owns rewriting.
- Accept records when they match either legacy policy or current configured policy.

## Unique Validation Path

Replace duplicated ad hoc checks with one owner:

```text
resolve_worktree_policy(project_root, config, raw_path)
  -> AcceptedPolicy { canonical_path, policy_version: Legacy | Configured }
```

Required callers:

- task register / dispatch `validate_worktree_path`
- task relocate `validate_worktree_path`
- cleanup `cleanup_worktree_path`
- handoff reference resolution
- migration worktree matching
- any CLI/MCP text that documents accepted paths

Do not add a fallback inside validation. If config is missing and the path is not a legacy record, fail closed with a typed error.

## Collab Code Nodes

1. Config node
   - Add `Worktree` config struct.
   - Validate base, layout, sanitization placeholders, and canonical base.
   - Include worktree config in effective config output.

2. Resolver node
   - Add one helper that canonicalizes a candidate against legacy root or configured rendered base.
   - Preserve existing basename/slug and escape checks.
   - Remove only the obsolete path-length behavior that conflicts with long configured base paths; keep slug-length limits.

3. Task lifecycle node
   - Task register/dispatch uses resolver.
   - Task relocate uses resolver.
   - Deliver and close continue comparing against the registered stored path.
   - Close cleanup accepts both policy versions because cleanup must not strand existing tasks.

4. Migration node
   - `worktree_matches_project` accepts legacy project playground and current configured path.
   - Existing migration tests remain semantically valid for legacy records.
   - Add tests for configured path acceptance and outside-base rejection.

5. MCP/CLI text node
   - Remove absolute claims that `--worktree-path` must be `<project-main>/playground/<short-slug>`.
   - State: configured base when configured, legacy path only for existing records.

6. Skill docs node
   - Update `collab/skills/collab/SKILL.md`.
   - Update `collab/skills/collab/references/task-worktree-lifecycle.md`.
   - Update embedded generated text in `collab/src/scope.rs`.
   - Update `collab/src/bin/collab-mcp.rs` and `collab/src/subagent.rs` CLI help strings.

## Tests

Targeted config tests:

- valid base and layout parse.
- missing/relative/non-canonical base rejected.
- invalid layout rejected.

Targeted resolver tests:

- legacy `<project>/playground/slug` still accepted.
- configured `<base>/<project-key>/slug` accepted.
- configured path outside base rejected.
- configured path using `..` rejected.
- configured symlink escape rejected.
- long configured base path accepted when slug remains short.

Targeted lifecycle tests:

- register accepts configured path and stores canonical path.
- relocate accepts configured path and rejects outside-base path.
- close cleanup accepts both legacy and configured stored worktrees.
- handoff reference accepts configured registered worktree.
- migration accepts legacy and configured records.

Runtime/E2E tests:

- run focused Collab tests in this candidate worktree.
- install candidate through official Collab release/install flow.
- verify installed digest.
- use one explicit `collab down` / `collab up` maintenance window.
- verify `collab context` still recovers identity from canonical project main.
- verify `collab-mcp initialize` succeeds.
- replay a live task register or equivalent configured-path validation through the installed CLI/server if a safe fixture project is available.
- if no safe fixture is available, record the exact non-applicable reason with evidence.

## Design DAG

```text
entry: approved policy + latest origin/main
  -> D1 inspect current config/schema/docs/tests
  -> D2 add typed worktree config contract
  -> D3 add unique worktree policy resolver
  -> D4 wire resolver into Collab lifecycle and migration validation
  -> D5 update CLI/MCP/Skill text from configured-policy language
  -> D6 add focused unit tests for config, resolver, migration, lifecycle
  -> D7 run focused tests in candidate worktree
  -> D8 build release candidate through official Collab flow
  -> D9 install candidate and verify digest/version
  -> D10 collab down
  -> D11 collab up
  -> D12 collab context from canonical main
  -> D13 collab-mcp initialize
  -> D14 live configured-path replay or explicit non-applicable reason
  -> D15 independent Codex review of verified candidate
  -> D16 fix review findings if any, then rerun affected tests/E2E/review
  -> D17 fetch origin/main and combine latest main with verified candidate
  -> D18 rerun affected gates after combine
  -> D19 merge to clean main if no remote advance/conflict/lock
  -> D20 push origin/main and record remote SHA
  -> D21 cleanup this task worktree only
exit: main pushed, runtime verified, no unclaimed task resources
```

## Failure and Cleanup Edges

- Config base unavailable before coding: stop and report missing prerequisite; do not use local fallback.
- Focused test failure: stay in candidate worktree, debug through root cause, rerun affected tests.
- Installed binary mismatch: do not restart daemon; rebuild/reinstall until digest matches candidate.
- `collab context` or MCP initialization fails after restart: keep daemon state, preserve logs, report INCOMPLETE.
- Review finds behavioral defect: return to D4/D5/D6 and rerun affected gates before D7.
- Merge blocked by remote advance, conflict, or lock: stop and report; do not force.
- Cleanup failure: keep worktree and report owner/path/reason; do not force-remove dirty resources.

## Exit Evidence Required

- Candidate SHA.
- Installed binary digest/version.
- Restart timestamp/PID or health evidence.
- `collab context` success from canonical main.
- `collab-mcp initialize` success.
- Live replay result or explicit non-applicable evidence.
- Independent Codex review PASS.
- Main merge SHA and `origin/main` remote SHA.
- `git -C <repo> worktree list --porcelain` no longer contains `/Volumes/Intel/playground/appsdk/worktree-playground-policy`.
- `test ! -e /Volumes/Intel/playground/appsdk/worktree-playground-policy` succeeds after worktree removal.
