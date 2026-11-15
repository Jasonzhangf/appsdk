# Project route ablation result

## Goal

Remove the host-root/resident-project route special case while preserving
registered project storage, identity, authorization, and durable work.

## Result

No source change was safe in this increment. The host is still a project
`Server`, not a host-only owner. Removing the resident branch would therefore
either route a project into host control storage or lose registrations whose
only durable evidence is in legacy reducer state.

## Evidence

- `ProjectRuntimeManager::new` derives `host_root` from `host.root`, builds
  some routes from `HostRouteRegistry::for_server(&host)`, and maps a resident
  route to `host.storage_root` and `host.clone()`.
  `collab/src/server/mod_parts/runtime_manager_setup.rs:17-80`
- Route replay treats a route whose project and storage roots match the host
  as the host reducer, restores its registration there, and rejects some
  mismatched resident records.
  `collab/src/server/mod_parts/runtime_manager_setup.rs:100-175`
- `HostRouteRegistry::for_server` reads registrations from
  `server.state.global.projects`; it classifies the project matching
  `server.root` as resident and other registrations as not ready.
  `collab/src/server/mod_parts/part_01.rs:91-160`
- The dispatch path directly runs a first project at `host_root` on
  `self.host`. Identity-context admission and project reset also compare
  against `host_root`; runtime creation skips the project writer lock for that
  root. These are ownership branches, not only route lookup optimizations.
  `collab/src/server/mod_parts/part_04/runtime.rs:17-74`,
  `collab/src/server/mod_parts/part_04/runtime.rs:102-140`,
  `collab/src/server/mod_parts/part_04/runtime.rs:258-343`,
  `collab/src/server/mod_parts/part_04/runtime.rs:1680-1735`,
  `collab/src/server/mod_parts/project_reset_manager.rs:119`
- Startup migration reads the legacy `service_scope_root`, copies selected
  host control fields, clears those fields from the legacy reducer, and
  ensures a route only for that legacy root. It does not migrate all legacy
  `GlobalState.projects` registrations into fixed route records.
  `collab/src/server/host_state.rs:92-200`,
  `collab/src/server/host_state.rs:245-298`
- `GlobalState.command_receipts` is stored in project `State` and is consumed
  for host-wide idempotency and project-scoped operation outcomes. Moving the
  host reducer or its commit path without classifying this mixed ownership can
  break replay guarantees.
  `collab/src/server/global_state_models.rs:762-780`,
  `collab/src/server/mod_parts/part_02/commit.rs:145-188`,
  `collab/src/server/mod_parts/part_04/binding_control.rs:265-292`
- The current startup-recovery graph validates as a closed SESE graph:
  `dagpipe graph validate docs/dagpipe/collab-startup-recovery.graph.json`
  returned `valid DAG` with 7 nodes and 6 edges. Its route node still
  describes routes as having no resident project, which is not yet true in
  the manager implementation.

## Next bounded change

Before deleting resident-route branches, add a migration-only route recovery
step under the existing host writer lock. It must project legacy
`GlobalState.projects[*].registrations` into ordinary `(app_scope_id,
project_scope)` route records and preserve each registration time. For each
record, preserve the existing storage owner from a matching route record or
an unambiguous existing state location. If storage ownership is missing or
conflicting, return a specific migration error before publishing routes.
Do not infer ownership from cwd, use host storage as a project placeholder, or
move project state. Verify migration with an isolated public CLI fixture
containing two registered projects with distinct existing storage, plus a
conflicting-owner case that fails without replacing either state. Then make
every route materialize an independent project runtime before removing the
resident comparisons. Host-only control ownership and mixed command-receipt
semantics remain explicit prerequisites to a real `HostRuntime`.

## Commands and observed outcomes

- Read the task contract, prior plan/review results, prior host-runtime
  increment result, project development Skill, affected source, tests, and
  startup-recovery graph.
- `dagpipe graph validate docs/dagpipe/collab-startup-recovery.graph.json`:
  passed; 7 nodes, 6 edges.
- No test or build was run because no source or test was changed. Previously
  recorded results in `host-runtime-integration-20261115-result.md` were not
  repeated.
- No production state, daemon, identity, message, tmux, or Git state was
  changed. The pre-existing dirty worktree was preserved.
