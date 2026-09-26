# Optional Black-Box Test Governance (AppSDK Owner)

AppSDK owns selection, scope confirmation, scenario contracts, runner registry
references, effect authorization, evidence binding, freshness, and the final
object test admission gate. DAGpipe CLI remains graph-only: it validates SESE
Graph topology and is never a substitute for test evidence or AppSDK admission.

## Mode and manifest

`test_governance` is optional in `.appsdk/project.json`. Missing or
`"mode": "off"` keeps existing DAG compile/run behavior unchanged. A selected
project declares a project-relative manifest path that must conform to
`contracts/test-governance.schema.json`.

The manifest contains only selected objects, scenario contracts, trusted
runner references, and effect authorizations. AppSDK never executes shell
strings from governance records. Scenarios refer to runners by stable
`runner_ref`; the actual project test entrypoint remains project-owned.

## Semantic graph contract

Every selected object must declare a Chinese business semantic graph in
`semantic_graph`. The graph is a single-source/single-sink (SESE) shape on
business states or observable behaviors: one `entry` node for the external
trigger, one `exit` node for the acceptable terminal state, Chinese business
labels on every node, and edges that keep every node reachable from `entry`
and co-reachable to `exit`.

Each scenario must map onto that graph with `path_node_ids`, a non-empty
traversable path that starts at `entry` and ends at `exit`. Missing, empty,
non-Chinese, multi-entry/multi-exit, unreachable, or unbound scenario paths
are explicit validation failures and block selected-object admission.

## Result records

Each selected scenario has a result record at
`.appsdk/records/test-scenario-results/<object_id>/<scenario_id>.json`
conforming to `contracts/records/test-scenario-result-record.schema.json`.
`passed` results must reference an EvidenceRecord whose `source_commit`
equals the current candidate SHA, whose result is `pass`, whose environment
and entrypoint match the scenario, and whose evidence is not expired.

Object admission passes only when all declared scenarios for that object have
current passed evidence, cleanup is complete or explicitly not required, and
any effect scenario has a matching unexpired authorization.

## CLI

```sh
appsdk verify --test-admission <project> [--object <object_id>]
appsdk verify --admission <project>
```

`--test-admission` lists selected object status without executing tests.
`verify --admission` applies the optional test governance gate only when the
project is selected. Ordinary `appsdk verify` reports `not_selected`,
`passed`, or `blocked` without making test passage a delivery requirement.
`compile` does not load the optional test manifest.
