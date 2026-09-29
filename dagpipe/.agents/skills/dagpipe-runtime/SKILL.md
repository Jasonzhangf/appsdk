---
name: dagpipe-runtime
description: Use the DAGpipe Rust SDK and global governance CLI to register project Operators, validate graph topology, and run compiled project pipelines; not for changing DAGpipe itself.
---

# DAGpipe SDK and governance CLI

Third-party projects use two complementary interfaces:

- Rust SDK: project-owned code defines and registers executable Operators;
  only an immutable `CompiledGraph` can enter the Runtime.
- `dagpipe` CLI: static governance modules inspect project graph files, validate
  DAG topology, and show node-to-Operator name/version bindings. The CLI does
  not load project code or execute project Operators.

## Model an existing project before changing it

When asked to architecture-manage or restructure an existing project, do not
return only a high-level DAG. First trace the real entrypoints, callers,
implementations, persisted/external resources, and existing tests. Produce a
bounded design slice that covers all applicable dimensions below, with current
owners and evidence; mark genuinely absent dimensions `not applicable` with a
reason instead of silently omitting them.

- **Identity and roles:** list the project/execution identities and the human,
  agent, service, or subsystem roles that initiate work, own each operation,
  approve effects, and receive outcomes. State each role's allowed
  responsibilities/capabilities and forbidden control (for example, Operators
  do not schedule nodes or rewrite topology). Do not equate a role with a
  process, worker, connection, or session.
- **Events:** enumerate triggering, control, lifecycle, and outcome events.
  For each, specify producer, consumer, when emitted, identity/correlation
  fields, state effect, and payload boundary. Keep control facts in events or
  typed state; pass business data through ARC values/references, not large
  event payloads. Include failure, cancellation, retry-request, and completion
  paths where applicable.
- **State machine:** name lifecycle states and terminal states; define each
  transition as `(current state, event) -> next state`, its owner/guard, and
  invalid transitions. Keep lifecycle loops such as retry outside the static
  DAG: a retry starts a new execution/attempt identity rather than making a
  graph cycle.
- **DAG and data contracts:** define the graph's trigger, nodes, dependency
  edges, ARC inputs/outputs, declared outputs, and success/failure/cancel
  terminals. Show which event or state transition starts the graph and how its
  result is consumed. For project audits, enumerate every business object
  source and validate its flow separately as a SESE Graph (one entry ARC, one
  exit ARC); the ARC payload schema is project-defined, not necessarily a JSON
  object. Do not combine independent sources into a multi-entry graph.
  Project-internal module implementation need not be a DAG; keep each graph at
  the exposed module/object dependency boundary rather than expanding functions
  into graph nodes.
- **Change boundary:** name the exact in-scope modules/files and callers to
  change, the out-of-scope neighboring systems/files, required compatibility
  or migration behavior, dependencies, and the tests/evidence that accept the
  slice. Do not turn an architecture map into blanket authorization to rewrite
  the whole project.

### DAG node granularity

Use an Operator-sized step, not an entire subsystem, phase, or vague label such
as `process data`. Each node must have one explainable responsibility and be
independently reviewable: identify its owner/role, exact Operator name/version,
input ARC(s), output ARC, side effects/capabilities, failure behavior, and
verification evidence. A node's success condition must be observable, not
merely “done”.

Split a step when it has a distinct owner, input/output contract, external
effect, retry/failure boundary, or independently testable result. Keep adjacent
micro-transformations together when splitting creates no independent contract,
effect, owner, or verification point. Put branching in explicit declared graph
structure; do not hide route changes, skips, or graph mutation inside an
Operator or Hook. List the concrete files/modules behind each changed node and
the callers/edges that connect them. If the real implementation does not yet
match the proposed graph, label the missing edge/implementation and its owner;
do not imply the proposed graph already executes.

### Semantic diagrams are mandatory

Present project DAGs and state-machine diagrams as **Chinese business-semantic
diagrams**, not code-navigation or function-call diagrams. Node/state labels and
edge/event labels must say what happens in the project, in Chinese. For example,
use `校验导入记录` and `记录校验结果`, not `validate_records()` and
`write_result()`; use `等待用户确认` --`用户确认导入`--> `正在导入`, not a
method name as a transition.

Do not use function names, method names, class names, source files, or call
relationships as the diagram's semantic nodes/edges. A DAG edge means a
business dependency or data/control fact (optionally label the ARC/result it
carries); a state transition means a declared business event changes lifecycle
state. Neither means “this function calls that function”. Mermaid or other
diagram syntax may use technical IDs internally, but visible labels remain
Chinese and semantic. If implementation traceability is needed, provide a
separate mapping table from semantic node/event to Operator and source owner;
that table is supporting evidence, never a replacement for the semantic graph.

The state-machine diagram must show meaningful Chinese lifecycle states and
the Chinese event/condition that causes each transition, including terminal,
failure, cancellation, and retry paths when applicable. Do not present an
English enum list or function jump chart as the state-machine design.

### Development and debugging with the DAG

For a feature or bug in modules that already have a DAGpipe design, first read
the relevant semantic DAG and state-machine diagram. Trace the expected
business path from its entry event through data/control edges to success or
failure terminals, then correct the graph against the business contract and
desired behavior. Persist the corrected model in the project-owned graph file
and validate it with `dagpipe graph validate` before comparing it with
implementation or editing Operators; a diagram kept only in chat is not the
design artifact. Do not treat current code as proof that the graph is logically
correct.

Once the graph is correct, compare each relevant node, edge, Operator binding,
and terminal with real code and execution evidence. Mark implemented, missing,
or stale parts, then investigate the first implementation divergence using
focused tests, ARC/journal facts, and the real entrypoint. Do not rewrite a
correct business graph to match faulty code or debug downstream from the final
symptom. If no graph exists, build and persist the smallest graph from the
business contract, then validate it before implementation. If code inspection exposes a missing real path, update the
implementation mapping; change the graph only when new contract evidence shows
the graph itself is wrong. For new development, use the corrected graph to
locate the exact node, dependencies, allowed files/modules, and acceptance path
before editing; do not expand into neighboring nodes without a demonstrated
edge.

### 交付生命周期 DAG（单源单汇）

DAGpipe 改造的交付不能停在代码、图或测试通过。每次影响运行时、客户端或发布的改造按单源单汇收口：`代码完成 → 交付生成物 → runtime 重建/重启 → worktree 回收 → playground 清理 → tmp 移除`。证据按序出示：产物版本/哈希与已验候选一致、运行 binary 哈希/重启时间/health 已证明、`git -C <repo> worktree list --porcelain` 不含本任务 worktree、`test ! -e <playground>` 成功、本轮临时文件/日志/forward/进程已移除。任一适用终点缺失标 `UNVERIFIED` 或 `INCOMPLETE`；清理只移除本轮自己创建且已确认不再需要的资源，不得删除他人 worktree、共享 playground、既有 dirty 文件或共享进程。用户已授权的目标项目常规交付内，**OTA 发布、git 提交、merge、push 不需要逐步询问**；按验证和 review PASS 自动执行并出示证据。

### SDK support boundary

The SDK provides a static acyclic data graph, ARC grants, Operator registration,
compile-time contract/capability checks, execution journal, hooks, bounded
node concurrency, and a separately callable declarative `StateMachine`. In
this MVP, the state machine is caller-owned: Runtime does not automatically
consume its events or launch a graph from a transition. Roles are project
architecture/governance facts, not a built-in authorization engine. State
machines may cycle across executions; an individual compiled DAG may not.

## Project integration

### Install and govern

Install the CLI, SDK source, and this Skill globally from the AppSDK checkout.
The crate is installed at `$HOME/.local/share/dagpipe/sdk` (the installer's
fixed per-user SDK directory):

```sh
scripts/install-global-dagpipe.sh
dagpipe --help
dagpipe modules list
dagpipe sdk path
```

In a consuming project, validate and inspect the graph JSON before running its
SDK compile gate:

```sh
dagpipe graph validate path/to/graph.json
dagpipe graph inspect path/to/graph.json
```

The CLI checks static topology, one declared source/input ARC and output ARC
per Graph (SESE), deterministic waves, ARC edges, and declared
`operator@version` bindings. It
intentionally cannot see the consuming project's Rust Registry or prove its
contracts/capabilities.

### Add and run the SDK

Use the absolute path printed by `dagpipe sdk path` in the consuming project's
`Cargo.toml`. Cargo does not expand `~` or `$HOME` inside TOML, so replace the
example home prefix with the actual path printed on your machine:

```toml
[dependencies]
pipeline_runtime = { path = "/Users/<your-user>/.local/share/dagpipe/sdk" }
serde_json = "1"
```

Implement business behavior in project-owned Operators; register them, build a
`Graph`, and compile it against the exact allowed effects. The minimal flow is:

```rust
use pipeline_runtime::*;
use std::collections::BTreeSet;

let mut registry = Registry::default();
registry.register(MyOperator)?;
let allowed_effects = BTreeSet::new();
let compiled = compile(graph, &registry, &allowed_effects)?;
let runtime = Runtime::new(allowed_effects);
let result = runtime.run(&compiled, identity, input_arcs, &Cancellation::default())?;
```

Implement `Operator::execute(input, context)` and declare its name, version,
input/output `ValueType`, and any external effects. Each node binds the exact
Operator name and version. `Node.inputs` and `Node.output` are its ARC grants;
Operators receive values, not the graph, scheduler, Runtime, or ARC store.
Configuration defines graph structure; it does not define executable code.

The SDK `compile(graph, &registry, &capabilities)` is authoritative for
Operator resolution, contracts, ARC access, effects, and DAG validity. Pass only
the resulting immutable `CompiledGraph` to `Runtime::run`. Supply project,
graph, execution, and attempt identity plus all declared input ARCs. Retry with
a new `execution_id` and `attempt_id`; never create a cycle to retry.

For a complete compilable example and graph schema, use
`dagpipe/docs/usage.md` and `dagpipe/examples/data_pipeline.rs` in the AppSDK checkout. The
installed Skill itself contains this quick-start and the runtime boundaries;
it does not require that repository to be present in the consuming project.

## Runtime invariants

- Runtime is single-process. Parallelism defaults to one; opt in with
  `with_max_parallelism(NonZeroUsize)`. Only independent DAG nodes overlap.
  Operators and node hooks must support concurrent calls when enabled.
- Use ARC edges to order operators sharing an external resource. Effect
  declarations are audit/capability metadata, not locks or a sandbox.
- A failed wave drains already-started nodes and may finish their effects;
  later waves do not start. Inspect `ExecutionFailure` and its journal.
- Journal order is deterministic logical order, not physical completion time.
- Pure Operators default to replayable. Effectful Operators default to
  `RequiresConfirmation`; explicitly declare a narrower replay guarantee only
  when its semantics justify it.

## CLI and Skill installation

Run `dagpipe --help` and `dagpipe modules list` to discover the static CLI
modules. From the AppSDK checkout, `scripts/install-global-dagpipe.sh` installs the global
binary and then its packaged Skill into `~/.agents/skills/dagpipe-runtime`.
The installer refuses to overwrite an existing different Skill version.
`dagpipe skill install` is available for a separate idempotent Skill install.

For examples and the full public API guide, see `docs/usage.md` in the DAGpipe
module. For changes to DAGpipe itself, use its invariant tests and all
executable examples; this usage Skill does not authorize runtime topology
mutation, automatic retries, or external project deployment.
