# Bootstrap and Migration

## Existing project: governance ablation

适用：已有项目收到消融授权，要采用 2026-10-16 版规则，删除过度保护、无独立
行为保障的检查及旧流程。这里迁移规则与执行入口，不重置项目控制面。
总禁令由 [治理 Skill](../SKILL.md#purpose-and-mandatory-boundary) 维护。

### 区分安装、项目迁移与程序修复

全局安装更新本机 `appsdk`、`project-memory` 与三个 SDK-managed Skills，不会
自动修改项目 AGENTS、Skills、CI/hooks。项目执行者读取本节，再修改实际生效
入口。旧命令仍强制退役检查时，要修唯一程序 owner；改文档不等于程序已修好。
全局正式入口为 AppSDK 仓库的 `scripts/install-global-appsdk.sh`，各项目不必重装。

### 删除旧要求及实际调用

先读当前项目需求、AGENTS、相关 Skill、受影响图和实际 CI/hooks。复用现有
笔记，只定位受影响入口，不全仓扫无关事实，不新建逐条审计账本。

| 旧要求 | 迁移动作 |
| --- | --- |
| 日常提取/核对 SHA、checksum、文件或补丁摘要 | 删除命令、必填字段及调用；使用必要 diff、行为结果与普通 Git/安装回执 |
| 候选身份清单、冻结材料、逐字节对账、补丁正反重建 | 删除日常入口和专属脚本、fixture、生成物引用；不以来源或交接完整性加回 |
| 每阶段再次校验记录摘要、完整性、任意有效期或未变输入 | 直接复用有效结果；实际行为、依赖或环境变化时仅补受影响验证 |
| 日常完成必须走 promotion、Active/Protected、FreezeRecord 或发布记录链 | 从日常路径移除；仅对外 release 准备且实际分发需要时使用 |
| 局部改动默认全仓/全平台测试、每节点 Review、差异证明账本 | 按实际失败模式选测；沿宿主 milestone Review 时点；关键取舍写现有笔记 |
| 只锁源码文本、私有结构、历史输出，或重复已有保障的测试/校验 | 确认用途后物理删除，清理专属脚本、fixture、依赖与失效引用 |
| 缺辅助状态阻断独立开发，或规则升级触发 Collab reset/重启 | 解除无关依赖；仅真实缺失能力阻断其下游 |

这是正式退役旧要求。同步修改规则正文、实际 CI/hook 调用和相关设计，不能仅
加一段相反说明。不得用 `--no-verify`、skip、吞错或补造 PASS 代替迁移。
保留同一入口中防止实际行为错误、安全问题或数据丢失的检查。

### 已有项目副本与程序门禁

- 项目 AGENTS 与自有 Skill 由项目修改。`.appsdk/skills/`、`.appsdk/templates/`
  是受管分发资源；需要刷新时走 SDK 正常升级入口，不手写 sdk.lock、pin、摘要、
  ReviewRecord 或迁移见证。不为规则消融运行 fresh/reset。
- `appsdk init` 只负责其初始化/资源刷新，不自动迁移项目 CI/hooks。仅在确需
  刷新资源且满足准入时使用。遇到 pin/版本等兼容错误保留原错并交 SDK owner
  修复，不反复 init 或删除目录。
- 现有 `produce-lifecycle-chain`、`verify --review-admission` 仍可能检查旧哈希
  记录。若调用只为日常身份包装服务，从项目 gate 正式退役该调用；若同时承担
  必需行为，则先在唯一 owner 消融冗余校验并保留该行为。不得将命令失败改报
  通过；SDK 缺陷沿现有 upstream bug 入口处理。
- 历史记录与已发布资产保持原状。退役要求和旧失败不改写成 PASS，不把旧证据
  复制成当前验收。业务数据或控制状态删除须有该操作的明确授权。

纯规则/文档走快速路径。程序、CI/hook 行为按项目现有开发流程，使用外置独占
worktree，先核对受影响 DAG，再做必要红测与公开入口回归。不另建治理框架。

### 完成条件

在日常任务实际入口确认：退役检查不再调用；受影响成功、失败和副作用符合需求；
有效证据可直接复用。只做受影响验证，不新增只检查规则关键词的测试。
普通提交/集成仍走正式保护入口。现有笔记记录关键删除项、保留保障、实际入口
结果和未完成项即可；项目副本或程序未改好时，明确列出 owner 与缺口，不能仅凭
全局安装成功结案。达到条件后交付并回收自有资源，不追加包装检查。

### 给项目执行者的启动提示词

```text
读取全局 appsdk-project-governance Skill 及 bootstrap-migration.md 的
“Existing project: governance ablation”章节，按当前项目需求执行旧治理消融。
检查实际生效的 AGENTS、Skills、设计、CI/hooks 和程序入口，删除对外 release
准备前的 SHA/checksum、候选身份包装、冻结材料、重复检查及无独立行为保障的
测试与治理。同步修改唯一 owner 和调用，不只改说明。
保留真实公开入口行为、安全和数据正确性验证，复用有效结果。纯文档直接改；
代码和 gate 行为走项目现有外置 worktree、DAG、红测及公开入口回归流程。
不要 reset/fresh、清空控制面、重启无关 daemon、伪造 PASS 或绕过 hook。
完成后报告实际删除项、保留的行为保障、入口结果和剩余缺口，按项目授权交付。
```

## New project

```text
requirements + acceptance
-> appsdk prepare
-> confirm project root/boundaries/non-goals
-> appsdk init
-> optional approved Guidance setup/compile
-> appsdk verify
-> clean owner worktree
```

`init` is idempotent. It fills missing governance resources and preserves
business files. AppSDK project initialization remains its own owner and may
invoke the daemon context internally when a live Codex App Server sessionID
binding exists. The agent must not rerun AppSDK initialization or run another
identity command to repair pending Collab. For agent-facing Collab bootstrap,
run `collab context` once. `registered: true` ends bootstrap. If the snapshot
returns `required_fields`, supply only those real facts once with
`collab context --provide '<JSON>'`. The supplement may contain only requested
`session_id`, `thread_id`, `endpoint`, or `namespace` facts; it never supplies
a worker, approval, token, route, or binding. The daemon owns identity
creation, selection, recovery, registration, route publication, and lease
restoration. If AppSDK reports Collab pending or context explicitly reports
daemon DOWN or a runtime error, preserve the exact result and stop; daemon
maintenance is human-authorized. Use `new` only for an empty destination.

`appsdk prepare` is a hard gate. First invocation writes `.appsdk-prepare.json`
with `status: "draft"`. `appsdk init` rejects an unconfirmed preparation with
`PREPARATION_NOT_CONFIRMED`; it does not guess scope or boundaries. Rerunning
`appsdk prepare` prints the existing record and does not overwrite it.

Before `appsdk init`, the operator must explicitly confirm these fields in
`.appsdk-prepare.json`:

```json
{
  "schema_version": 1,
  "preparation_id": "prepare-<slug>",
  "status": "confirmed",
  "objective": "One-sentence confirmed goal for AppSDK governance admission.",
  "change_kind": "new_project",
  "project_root": ".",
  "legacy_roots": [],
  "new_roots": [".appsdk/**", ".appsdk-control/**", "playground/**"],
  "protected_roots": ["protected/**", ".git/**"],
  "runtime_forbidden_roots": ["generated/**", ".agent-collab/**"],
  "boundary": {
    "allowed_paths": [".appsdk/**", "playground/**", "active/lib/**", "protected/**"],
    "forbidden_paths": [".git/**", ".agent-collab/**", "dist/**"],
    "payload_control_separation": "confirmed"
  },
  "acceptance_criteria": ["appsdk init completes", "appsdk verify passes"],
  "non_goals": [],
  "questions": [],
  "confirmed_by": "Jason (explicit user approval)",
  "confirmed_at": "2026-09-15T00:00:00Z"
}
```

`change_kind` must be one of `new_project`, `module_refactor`,
`project_refactor`, or `debug`. `project_root` is the relative AppSDK project
root from the preparation file location; `.` means the same directory. All open
questions must be answered or removed before `init`. Never confirm the record
on behalf of the user, and never replace the preparation gate by editing
`.appsdk/project.json` before initialization.

For an existing project, ordinary rule upgrades use
[governance ablation](#existing-project-governance-ablation), not the new-project
or reset path. Only an explicitly authorized control-plane discard uses
[Existing project: remove old governance](#existing-project-remove-old-governance),
with AppSDK and Collab transactions kept separate.

The existing preparation fields also bind the skeleton and operator plan. Use
`objective`, `boundary`, `acceptance_criteria`, `non_goals`, and `change_kind`
directly. When the project has a real business flow, also follow
[Skeleton, Operations and operators](#skeleton-operations-and-operators).
Do not add a parallel intake, schema, registry, or approval checklist.

AppSDK preserves the launching environment and does not pass a project path to
Collab. The daemon resolves project scope from the exact process cwd. Without a
live registered App Server transport, AppSDK initializes governance and
reports Collab pending because no peer can be registered; it never fabricates
subscription state.

Collab bootstrap errors from AppSDK's internal context invocation are explicit
warnings for AppSDK initialization. Automatic multi-worker registration and
task/file coordination remain enabled; shared operations wait for reliable
ownership while independent work continues.

### Master and ordinary peer bootstrap

For a project that will run multiple agents, initialize the AppSDK governance
root first, then run one `collab context`. Through that invocation the daemon
registers/establishes the current peer and its lease; role assignment is
human-approved and separate.

Master initialization, after user approval for the exact project and peer:

```bash
cd /abs/path/project
appsdk prepare
# confirm .appsdk-prepare.json exactly as above
appsdk init .
appsdk guide status
appsdk verify
collab context
# if required_fields are present, provide only those facts once
# only with explicit approval for this exact peer/project; complete the
# current takeover confirmation when replacing a different online holder:
collab master promote --approval "<user approval text>"
collab context
```

Do not promote without the user's explicit approval. An approved takeover can
replace a live or offline holder; liveness is not authority. `appsdk init` alone never
creates master authority. If `appsdk init` reports Collab pending or a failed
registration, preserve the exact error; do not report Collab as available and
do not start a second daemon.

Ordinary peer registration or recovery uses the current project and caller
context. It does not require a reachable master or a consumer project contract:

```bash
cd /abs/path/project
collab context
# if required_fields are present, provide only those facts once
```

The peer must observe its own identity, liveness, presence, transport and peer
role. Do not run a second identity bootstrap, do not promote itself, do
not fabricate a worker role from AppSDK initialization output. An ordinary peer
is an autonomous goal orchestrator, not an execution subworker.

For accepted long-horizon work, master and peer each create their goal document,
then register and verify their own goal. Master does not register a peer's goal:

```bash
appsdk goal subscribe --goal docs/goals/<goal>-plan.md --interval 10m
appsdk goal status --json
appsdk longhorizon show --json
```

`appsdk goal status --json` must report `active: true`, `desired: subscribed`,
`observed: subscribed`, `collab_subscribed: true`, a non-null
`subscription_id`, and a null `error`. A successful command output is not
proof that a timer fired. Exercise the declared interval and record the
armed subscription, fired deadline notification, and consumed result before
declaring long-horizon scheduling verified. New periodic long-horizon goals
default to100 deliveries, with an explicit count taking priority. Reuse a valid
batch across consumption and restart. Renew an ended batch only while work and
authorization remain. Explicit one-shot deadlines and existing one-shot records
keep their lifecycle. A batch ending does not establish task or goal completion.
Master dispatch task sheets carry the long-horizon content and cadence, and
dispatch starts the target peer's subscription atomically. The current typed
CEO can modify any member's long-horizon schedule across projects.

For a new governance root, AppSDK installs a project-neutral root `AGENTS.md`
when none exists. It contains the Project Truth, Semantic Invariants,
Ownership, Architecture Truth, Development Process Control, Git Protection,
Task Routing, and Evidence Boundary sections used by Guide setup. Customize
the bracketed project facts through the approved setup flow. Existing project
rules are never overwritten, and rerunning `init` on an already governed root
does not recreate a deliberately absent `AGENTS.md`.

## Repeat initialization and template upgrade

Initialization is not one-shot. After AppSDK is updated, or when the project
wants to revisit its process, rerun `appsdk init` to refresh AppSDK-owned Bundle
resources. It installs the current versioned standard reference at
`.appsdk/templates/minimal/AGENTS.md` while preserving the project-owned
`AGENTS.md`, local Skills, machine Guidance, lifecycle records, Active, and
Protected state.

```text
appsdk init
-> Agent reads effective upstream rules, project AGENTS/Skills, test commands,
   and CI/hook entrypoints
-> when Guidance is selected:
   appsdk guide init --task guidance-upgrade --mode bootstrap --module <id>
   compare retained rules and useful differences
   GuidanceSetupProposal
   reuse existing session authorization; approve only uncovered differences
-> otherwise: perform the same audit directly
-> latest origin/main clean owner worktree
-> apply authorized changes
-> when Guidance is selected: appsdk guide compile
-> appsdk verify
```

The template is a standard reference, not an active rule source. Do not add it
to `.appsdk/project.json#/guidance/rule_sources`, automatically overwrite
project rules, or reset valid governance merely to adopt a newer template.
The proposal records retained project rules, recommended changes, and declined
template items so choosing not to adopt an item is explicit and valid.
Guidance is optional for this audit. Repeated initialization and unrelated
version refreshes do not trigger a whole-project rule audit.
Missing or locally removed reference material does not fail ordinary
`appsdk verify`; rerun `appsdk init` only when a fresh comparison is wanted.

## Governance exists but Guide is missing

Do not reset or migrate valid lifecycle truth merely to add Guide. Run
idempotent initialization so the current AppSDK can install missing Guide
resources while preserving the existing project contract, maps, records,
Active, and Protected state.

An already governed root containing `.appsdk/project.json` may rerun
`appsdk init` directly; its existing project root is the authority, so a new
preparation record is not required for this non-destructive resource refresh.
Fresh or relocated initialization still requires confirmed preparation.

```text
appsdk init
-> appsdk guide status
-> GUIDANCE_SETUP_REQUIRED
-> appsdk guide init --task guidance-setup --mode bootstrap
-> Agent reads returned AGENTS and local Skill candidates
-> Agent asks only unresolved questions
-> Agent presents GuidanceSetupProposal
-> reuse session authorization; approve only uncovered differences
-> clean owner worktree updates AGENTS/local Skill/machine contract/source declaration
-> appsdk guide compile
-> appsdk verify
```

Bootstrap intake is read-only and may be invoked again after Guidance has been
compiled. Candidate files are not compiled rule sources
until the user approves them and `.appsdk/project.json` declares them. A
task-level PlanProposal is not a substitute for this project-level setup and is
never copied into a Skill automatically.

## Existing project

Use this section only when retained control state needs a canonical migration or
the user explicitly authorizes a named destructive reset. An ordinary rule
upgrade uses [governance ablation](#existing-project-governance-ablation).

For that migration or reset scope, inventory AppSDK roots, maps, records,
Active/Protected, local control state, claims, and worktrees. Choose one route:

### Preserve and migrate

Use when historical evidence remains valuable and the current version has a
supported canonical migration.

```text
snapshot immutable truth
-> reconcile ownership/conflicts
-> run canonical migration once
-> verify one retained truth
-> compile Harness rules
```

### Reset and reinitialize

Use when old governance is obsolete, unsupported, or costs more than its audit
value. Reset is destructive and requires user authorization for the named
objects.

```text
inventory + immutable audit snapshot
-> classify retained business source and Protected artifacts
-> request exact reset/delete authority
-> clean non-main owner worktree
-> appsdk init <project> --fresh --discard-legacy
-> old .appsdk audit/migration records and generated projection removed
-> current .appsdk contract, record contracts, and transition manifest rebuilt
-> rebuild maps/goal/module/owner from current project truth
-> appsdk guide compile
-> appsdk guide init for the current task/domain
-> appsdk verify
```

`--fresh --discard-legacy` is an explicit existing-project initialization route,
not ordinary `init` behavior. It requires `.appsdk/project.json`, a clean
non-`main`/`master` worktree, and the discard confirmation. It preserves
business source, runtime data, `active/`, `protected/`, and human documents;
only the named AppSDK control plane and declared generated roots are removed.
The current SDK scaffold is the only reset baseline: legacy SDK pins, migration
witnesses, indexes, and SDK-owned contract projections are ignored and
regenerated; missing SDK-owned fields are refilled. Project identity, module
ownership, build, and protection boundaries are carried forward.
The new reset record has `mode: "fresh_init"` and proves the reset operation
only. It does not inherit old PASS, review, delivery, or freeze claims.

Before choosing this route, install the reviewed current AppSDK and Collab
versions globally and use only those installed binaries and Skills for the
inventory. Do not read, replay, or interpret old local control history to make
the old baseline compatible. The current version is the only reset baseline:
missing SDK-owned fields are refilled, legacy SDK pins/migration witnesses are
ignored, and old local control state is removed only through the canonical
owner.

The replacement order is:

1. Install the reviewed AppSDK and Collab binaries/Skills.
2. Inspect with the newly installed commands and record the exact old control
   roots and owner.
3. Retire or migrate Collab through the Collab owner.
4. Reset AppSDK through the AppSDK owner in a clean non-main worktree.
5. Validate the new baseline only. Do not import old PASS, receipts, review,
   install, restart, or delivery evidence.

If a legacy user-local Collab binary pair is proven by its own version
response, removal requires explicit user authorization naming the exact binary
paths. After the canonical pair is installed, remove only those authorized,
verified paths. Never remove `~/.appsdk`, `~/.collab`, project
`.agent-collab/`, or business source as part of binary cleanup.

The lower-level `appsdk reset-governance <project> --discard-legacy` command remains
available and uses the same transactional reset owner. Neither command
authorizes manual deletion or hand-editing of version/hash/ReviewRecord, and
neither permits two active governance roots. If old Active/Protected artifacts
are also obsolete, name exact paths and authorize a separate cleanup.

### What to do with old reports and delivery output

Use ownership and rebuildability, not age, to decide what is removable:

| Class | Default action | Reason |
| --- | --- | --- |
| `.appsdk/records`, `.appsdk/transactions`, audit/migration reports | Inventory/snapshot if needed, then remove through reset | Old control truth must not leak into the new baseline. |
| Declared `governance.generated_root`, module generated outputs | Remove through reset and regenerate | These are reproducible projections, not source or release truth. |
| Failed transaction staging | Canonical abort/retry if current; otherwise reset | Manual deletion can hide ownership or partial publication. |
| `active/`, `protected/`, runtime data, business source | Retain | They may be the only published or operational truth. |
| `dist/`, `.deploy/`, `build/`, `tmp/`, custom reports/artifacts | Keep until exact disposable ownership is confirmed | AppSDK cannot infer that an external output is safe to delete. |

The reset command reads and validates the old project contract before removal,
carries that contract into the new `.appsdk` root, and includes its declared
generated root in the disposable set. It does not use a fixed project path or
silently delete Active/Protected. For external outputs, the
owner must name the exact path, establish that it is rebuildable, authorize
cleanup, and record the result separately. Never preserve an old report by
renaming it as a new record, and never make a new record by editing an old
hash or receipt.

## Mid-development adoption

Do not force release/freeze evidence onto unfinished work.

```text
read current effective source and task state
-> initialize advisory governance
-> bind current goal/module/owner/worktree
-> if Guide is missing, complete the user-approved setup proposal first
-> run task guide init, read declared AGENTS/Skills, ask unresolved questions
-> place workflow at the current real phase
-> apply new rules to new/changed nodes
-> continue development
```

Untouched legacy gaps are warnings unless safety, source ownership, evidence
truth, or current delivery is affected.

For changes to the project skeleton, Operations, operators, or graph
configuration, use [Skeleton, Operations and operators](#skeleton-operations-and-operators)
after placing the task at its real phase. Do not repeat that procedure here.

## Skeleton, Operations and operators

This section owns the project procedure for turning a business flow into a
skeleton, Operations, reusable operators, and graph configuration. Other
references route here; do not copy the procedure.

AppSDK governs consumer projects through this procedure and the mechanisms it
provides. The consumer owns its architecture, business semantics, operator
implementations, configuration, and acceptance. DAGPipe owns graph compilation
and execution. AppSDK's own Operations can serve as pilots; migrating one SDK
command does not by itself establish or verify governance for other projects.

Use this procedure to design the consumer's adoption. When a consumer turns
an instruction into a mandatory governance rule, use the
[ontology rule template](ontology-governance.md) to model its Object, Behavior,
and Relation, record exact clause approval, and bind its executable edge or
gate with legal and illegal cases and a real observation path. Keep proposed
clauses marked as candidates until those conditions are met.

### Shape the skeleton

1. Bind the user goal, accepted behavior, acceptance criteria, non-goals,
   owners, and allowed/forbidden paths first. For a new project, reuse the
   existing `appsdk prepare` fields. Do not add a second preparation record.
2. Map the whole project at skeleton level: modules, owners, Operations,
   public entrypoints, external dependencies, success and failure endpoints,
   and side effects. Mark unmigrated paths; refine and migrate each selected
   Operation incrementally.
3. Split a node only at a stable capability boundary. Do not turn ordinary
   functions into nodes mechanically. A function stays in the owner that
   provides the capability.
4. Keep configuration and control truth separate from business payloads,
   metadata, debug logs, and implicit context.
5. A missing required capability returns an explicit error on the affected
   path. An optional capability may be skipped only when its contract permits
   the skip. Preserve the actual result and error.

An Operation is one user-visible business action with an input, result, error
boundary, and declared side effects. It can compose several graph nodes. A
node invokes one capability in that flow. An operator is the versioned
implementation bound by the node. Define each selected node's purpose, inputs,
outputs, errors, side effects, and capability owner. Maintain reusable
operators with their capability owners instead of duplicating them per node.

### Define and verify capabilities

```text
declare Operation and node contract
-> reuse the existing operator or shared capability
-> define black-box success, failure, and side-effect assertions
-> implement or extend only the missing capability
-> verify the base capability
-> compile the graph and bind the real entrypoint
-> run the public acceptance path
```

Define the acceptance before implementing a new operator. A graph-only change
may reuse an already verified operator. A capability change updates its unique
operator owner and only the affected bindings. Verify each added or changed
operator through its public interface or a real consumer before wiring it.
The project's real entrypoint registers the operators and executes the compiled
graph; a valid design graph alone does not prove that this happens.

For a Rust consumer, obtain the `pipeline_runtime` library using the project's
actual dependency source, then follow the [DAGPipe usage guide](https://github.com/Jasonzhangf/appsdk/blob/main/dagpipe/docs/usage.md)
for operator implementation, graph configuration, registration, compilation,
execution, and output and failure handling. The guide is hosted in the AppSDK
source repository; the AppSDK Skill installer does not install DAGPipe's Rust
library or its documentation. The notification example below demonstrates an
existing SDK entrypoint, not registration of a consumer's own operator. Verify
other language integrations against their actual supported interface.

The runtime's `Object` type does not validate required business fields; those
remain the capability owner's contract. Multiple input ARCs in `Whole` mode
arrive as an array. An operator returns string errors wrapped by the runtime;
check that the consumer entrypoint can preserve its required error contract.
The graph has no general per-node business-parameter field. A consumer may
construct an operator from declared project configuration before registration.

### New project

Use the existing preparation fields to bind the modules, Operation, entrypoint,
external ARCs, operator owners, and acceptance. Add a graph only when the
actual business flow needs composition. A new project does not need a general
operation scheduler, a second registry, or a generated business graph that has
no real consumer.

### Existing project or mid-development change

Read the current behavior and affected graph first. Record the original
entrypoint, candidate entrypoint, and the terminal for switching or deleting
the old implementation.

```text
current behavior and affected graph
-> candidate implementation
-> same-input white-box and black-box comparison
-> switch the public entrypoint
-> delete the old implementation, call sites, dedicated tests, and dependencies
-> unique-entrypoint regression
```

Update the affected skeleton and Operation contract before changing the
implementation. Explain intentional old/new behavior differences. Reuse valid
evidence and recheck only the affected guarantees. A combination change updates
the graph. A capability change updates the owning operator. Do not keep the old
production path as a fallback.

One invocation keeps its selected compiled graph. If the business contract also
requires configuration to remain stable during that invocation, the consumer
owner must preserve the configuration and operator instance for its lifetime;
prefer an immutable configured instance instead of mutating a shared instance.
A fixed graph alone cannot keep external configuration or a process alive.
Do not add in-flight hot switching unless a later contract requires it. Arrange
state migration only when the state contract changes, and define how old
invocations complete or exit during a state-format change.

A failure still allows the other nodes in its scheduled wave to finish, even
with the default single worker. Later waves do not start. When multiple nodes
in that wave fail, the primary error follows stable node-ID order, not failure
time or an implicit business priority. Cancellation does not interrupt a
blocking operator or an `Items` iteration; an effect may still complete. Put
an action that must not run after a failed check in a later wave through a real
dependency. Effect capabilities are declarations and compile-time checks, not
locks, OS permissions, a sandbox, automatic retry, or rollback. If these runtime
boundaries cannot preserve a required error, cancellation, or transaction
contract, keep the existing atomic owner boundary and redesign that connection.

### Ablation and evidence

Physically remove an implementation, call site, dedicated test, or dependency
when no current consumer or independent guarantee needs it. Keep safety,
data-correctness, and public black-box checks.

Use `compile` for graph structure, types, bindings, and capability checks. Do
not repeat those checks at every layer. Do not add a per-item proof table, a
second governance status, hashes, or frozen snapshots merely to record an
ablation.

### Notification configuration example

The notification validation chain demonstrates this composition pattern.
Graph paths resolve within the project. Relative paths start at the project
root; an absolute path must also name a file inside that project.

```sh
appsdk dagpipe validate-notifications <project>
appsdk dagpipe validate-notifications <project> --graph <path>
```

| CLI choice | Successful result meaning |
| --- | --- |
| no `--graph` | `evidence_scope: "sdk_default_contract"`; the input passed the SDK's complete default notification contract |
| explicit `--graph` | `evidence_scope: "selected_project_program"`; the input passed the selected project program and entry-result consumption contract |

The default graph is
`source_validate -> delivery_validate -> terminal_accept`. A complete
alternative graph may order `delivery_validate` before `source_validate`.
These two complete graphs retain all validation stages. On an object with both a
source error and a delivery error, the first reported business error follows
the selected order.

An explicit graph is a trusted project program. A legal graph that omits source
or terminal acceptance may complete successfully. Its success remains
`selected_project_program`. SDK default acceptance uses the default command
and `sdk_default_contract`.
Derive `evidence_scope` only from the CLI default/explicit choice. Never infer
it from Graph ID, version, path, node names, or output content.

Default and explicit graph failures must fail explicitly. Do not fall back to
another graph or an old implementation. Consume the actual Runtime output; do
not rebuild a missing terminal from input state. Verify these behaviors
through the public command when changing this Operation.

After argument parsing and safe project-root resolution, this command appends
one invocation and outcome to `.appsdk-control/command-invocations.jsonl`.
The record contains the command, UTC start time, elapsed milliseconds, graph
selection, outcome, and a fixed error code on failure. It excludes message
content, graph content, paths, and business results.

Mailbox, graph, and other business files remain read-only. The invocation log
is the declared audit write. A log failure reports
`COMMAND_INVOCATION_LOG_WRITE_FAILED` on stderr and preserves the original
business result and exit status. Such a warning identifies an audit gap.
Early usage or unsafe-root rejection writes no project log. This logging
boundary covers the notification command; other CLI commands keep their own
owners. The log supports review and has no role in validation or recovery.

## Existing project: remove old governance

Use this section when a real project root already contains `.appsdk/`,
`.appsdk-control/`, or `.agent-collab/` from an older version and the operator
wants to start the current governance and coordination baseline instead of
migrating old control state. The two roots have different owners and must be
handled in separate transactions.

### 1. Inventory and freeze

Run read-only inventory from the project root. Do not delete anything yet.

```bash
cd /abs/path/project
git status --short --branch
git worktree list
find .appsdk .appsdk-control .agent-collab -maxdepth 3 -print 2>/dev/null
collab context
```

Record the exact project root, branch/HEAD, worktrees, `.appsdk/` and
`.appsdk-control/` contents, generated roots, `active/`, `protected/`,
the current Collab identity map, task/mailbox state and resource claims, daemon
PID/socket, peer identities, routes, and actual blockers in the run note. Collab
has no migration-maintenance command; current recovery is `collab context`.
A file name, old
PID file, socket existence, or successful `collab status` is not sufficient
proof of ownership.

Stop new shared writes, dispatches, and task admission before either reset.
Preserve every active worktree and task until its owner or an explicitly
authorized migration decision resolves it.

### 2. Collab migration or retirement

`.agent-collab/` is owned by Collab. AppSDK reset does not remove it or require
Collab migration. When the authorized plan actually changes Collab state, use
the [migration Skill](../../appsdk-migration/SKILL.md) and current Collab
contract. Do not duplicate a legacy offline reset transaction here.

The current project's master may reset that project's Collab assets online
through `collab reset --project`. Global daemon lifecycle is user-managed and
is not a reset prerequisite. Do not expose lifecycle commands or credentials
in agent prompts. Never manually edit journals, clear mailboxes, copy tokens,
or start a second daemon. Preserve the actual reset/migration receipt; neither
operation proves delivery or communication.

### 3. AppSDK reset

Handle `.appsdk/` from a clean non-`main` owner worktree with no competing
claim. A separate Collab operation is needed only if the authorized plan
changes Collab state. For a project that must abandon the old governance epoch and
start from the current SDK baseline, the preferred single entry is:

```bash
cd /abs/path/project
git worktree add -b codex/governance-reset-<slug> \
  /Volumes/Intel/playground/<project-key>/<task-slug> origin/main
cd /Volumes/Intel/playground/<project-key>/<task-slug>
appsdk init "$PWD" --fresh --discard-legacy
appsdk guide init --task governance-reset --mode bootstrap --module <module-id>
appsdk guide compile
appsdk verify
appsdk compile
```

`<project-key>` and `<task-slug>` come from the current project and task. Do not
reuse another task's worktree path.

`appsdk init --fresh --discard-legacy` requires an existing
`.appsdk/project.json`, a clean non-`main`/`master` worktree, and the explicit
discard confirmation. It uses the same transactional reset owner as
`appsdk reset-governance <project> --discard-legacy`, but combines reset with current
contract rebuild. It removes the old AppSDK control plane, `.appsdk-control/`,
and declared rebuildable generated roots; it preserves business source,
runtime data, `active/`, and `protected/` by default. It must not remove
`.agent-collab/`.

If the operator explicitly chooses the lower-level AppSDK operation instead,
run it in the same clean non-`main` worktree and then initialize:

```bash
appsdk reset-governance "$PWD" --discard-legacy
appsdk init "$PWD"
appsdk guide compile
appsdk verify
```

Do not hand-edit `.appsdk/` JSON, reuse old PASS/hash/receipts, or delete
`active/` or `protected/` as part of reset. Those are separate authorized
cleanup decisions.

If the old `.appsdk/project.json` is missing, malformed, unreadable, or the
worktree is dirty, `--fresh --discard-legacy` must fail closed. Do not replace
that check with manual deletion. Preserve the exact error and use the AppSDK
migration owner to establish whether the project contract can be recovered;
only an existing, valid contract can authorize a fresh reset. If no contract
can be established, a new root must go through a separate confirmed
preparation/init flow rather than claiming to reset the old project.

### 4. Verify the new baseline

The operation is complete only when all of the following are true:

- Collab reports the exact migration/retirement result and has one verified
  daemon/socket/identity state.
- `appsdk verify` passes against the current project contract and the fresh
  reset baseline.
- The removed classes are limited to the authorized AppSDK control plane,
  `.appsdk-control/`, and declared rebuildable generated roots.
- Business source, runtime data, `active/`, `protected/`, and all retained
  Collab evidence still exist.
- The new reset record is current and does not claim delivery, review,
  install, restart, or communication success.
