# AppSDK Project Integration

AppSDK is an external governance implementation. A new project consumes its CLI/compiler and commits only project-specific governance data.

## Repository boundary

### Module dependency artifacts

Declare modules in dependency-first order. During development, `compile-module`
requires each dependency's compiled artifact to exist and match its current
module contract and outputs. Missing or stale dependency artifacts fail
explicitly; compile the dependency first, or use the ordered project `compile`
command. Self-dependencies and reverse dependency edges fail before compilation.

Development compilation produces manifests and artifacts for consumption; it
does not alter Active/Protected or manufacture freeze records. Immutable
artifact freeze, formal admission, and publication records belong to external
release preparation (see [playground-active-promotion.md](playground-active-promotion.md)).
If a clean checkout is missing a retained frozen projection, restore it with
the explicit `appsdk rehydrate-frozen --module <id>` command.

```text
external AppSDK installation
  -> versioned Bundle: CLI / compiler / contracts / docs / rules / skills
  -> .appsdk project contract + sdk.lock
  -> compiled manifest and verified artifact
  -> project runtime consumes Active
```

The business project must not copy AppSDK source, compiler, or harness into its own source tree.

Project-scoped CLI commands use the process `cwd` as the default project root.
An explicit project argument is optional for out-of-directory operator use.
Projects and Skills must not require a project-root environment variable, and
help output must remain available without loading project state.

## Worktree and development environment

Mutations run from a clean isolated worktree following the SESE (Single-Entry Single-Exit) Close-Before-Open lifecycle under `/Volumes/Intel/playground/<project-key>/<task-slug>`. The main worktree is an inspection and integration surface. AppSDK mutation commands reject running directly on branch `main`; `verify` remains read-only.

Ordinary development, testing, and milestone delivery rules reference the unique global AGENTS and project skills ([`appsdk-dev`](../../.agents/skills/appsdk-dev/SKILL.md), [`coding-principals`](../coding-principals/SKILL.md)). Do not replicate a second set of development phases or preflight ceremonies here. Governance readiness provides mechanisms for project contracts, not blockers that interrupt business debugging.

Collaboration coordination and quality gates are separate:
- Collab (`collab context`) coordinates multi-worker tasks and task ownership. Collab status never blocks independent single-worker implementation, testing, or quality gates.
- AppSDK governance state (`.appsdk/`, `.appsdk-control/`) and Collab state (`~/.collab/`, `.agent-collab/`) have distinct owners.
- Control-plane migration, daemon maintenance, or authorized state resets follow dedicated procedures in [`appsdk-migration`](../appsdk-migration/SKILL.md) and [collab-control-plane-reset-20261005.md](collab-control-plane-reset-20261005.md), rather than ordinary development bootstrap.

## Project layout

```text
project/
  .appsdk/
    project.json
    goal.json
    sdk.lock
    maps/
    architecture/
    verification/
    records/
  .appsdk-control/       # local only; ignored
  playground/experiments/
  active/lib/
  protected/source/
  protected/contracts/
  protected/history/
  generated/              # indexes or project-declared artifact surface
```

### Path ownership: project-owned vs SDK-managed

- **Project-owned paths**: Maintained by the project team. SDK validates schema, references, scope, and lifecycle constraints without overwriting them:
  - `.appsdk/project.json`, `.appsdk/goal.json`, `.appsdk/requirements.json` (when present);
  - `.appsdk/sdk.lock`;
  - `.appsdk/maps/**`, `.appsdk/architecture/**`, `.appsdk/verification/**`, `.appsdk/records/**`;
  - Project source, tests, Active/Protected archives, and business documentation.
- **SDK-managed paths**: Installed and updated by AppSDK from the versioned Bundle:
  - `.appsdk/contracts/**` (machine contracts from Bundle manifest);
  - `.appsdk/docs/**`, `.appsdk/rules/**`, `.appsdk/skills/**`;
  - `.appsdk/sdk-resources.json` (auto-generated resource catalog; manual edits fail-fast in `verify`).
- **Local control paths**: `.appsdk-control/` is disposable local runtime state (temporary harness output, local caches). It must be ignored in Git and never contains project truth.

## Git policy

Commit:

- `.appsdk/project.json`;
- `.appsdk/goal.json`;
- `.appsdk/requirements.json` (when authoritative requirements exist);
- `.appsdk/sdk.lock`;
- `.appsdk/sdk-resources.json`;
- project resource/function/mainline/verification maps;
- project architecture and verification contracts;
- Active and Protected content when the project uses Git as its artifact/archive store;
- Evidence, Review, Promotion, and Freeze records when created for formal release.

Ignore:

```gitignore
.appsdk-control/
```

## SDK Bundle and lock

消费者只使用全局安装的 AppSDK Bundle。Bundle 版本必须同时绑定 Rust CLI、machine contracts、文档、规则和 skills；不能让项目自行扫描一个不受版本控制的全局目录。Bundle manifest 是机器真源，`init`/`new` 将其中声明的 Skill、规则和文档安装到项目 `.appsdk/`，并生成 `.appsdk/sdk-resources.json`。

项目中的手动合同（例如 `project.json`、records、maps）由项目维护，SDK 按相应入口校验结构、引用、owner 和 scope。`.appsdk/sdk-resources.json` 由资源 owner 写入。旧完整 Bundle 记录仍有其核心 `verify` 约束；下述 resource-only 刷新使用独立的 schema 2 读写路径，不运行旧摘要链，也不宣称核心版本迁移完成。

`contracts/sdk-bundle.manifest.json` 声明当前 Bundle 的资源集合和安装位置；
初始化后项目内的机器真源位于
`.appsdk/contracts/sdk-bundle.manifest.json`。`init`/`new` 直接把当前
Bundle digest、manifest digest 和资源集合写入锁。`pin-lock` 只用于受支持
的旧版本 migration；该命令仍要求执行中的 binary 与 `--binary` 指向的文件
字节一致，防止旧 CLI 把新 binary 与旧 embedded Bundle 拼成伪迁移。

`.appsdk/sdk.lock` binds the project to the external implementation:

```json
{
  "sdk": "appsdk",
  "version": "0.1.6",
  "bundle_digest": "sha256:<current-bundle-digest>",
  "bundle_manifest_digest": "sha256:<current-manifest-digest>",
  "bundle_resources": {"contracts": [], "docs": [], "rules": [], "skills": []},
  "contract_schema": 1
}
```

The lock is committed and is not bound to the bytes of the currently running
AppSDK executable. Optional `digest`, `compiler_digest`, and `binary_ref` fields
may remain as historical migration witnesses, but they do not gate ordinary
development. `verify` always checks the project version/schema and exact current
Bundle/manifest/resource binding.

### SDK-managed resource refresh

```bash
appsdk resources refresh [project]
```

The project path is optional and defaults to the process current directory.
This command refreshes only the embedded Bundle `docs`, `rules`, and `skills`
resources. It preserves `.appsdk/contracts/**`, `.appsdk/project.json`,
`.appsdk/sdk.lock`, requirements, goals, the project Guidance machine contract,
maps, records, migration history, Active, Protected, and business state.

The command reads the installed manifest and resource record before writing.
It resolves every target through the existing source/class/path mapping and
preflights all targets before the first resource write. The resource record is
written last. This is not a cross-file transaction; a write failure returns an
error and does not publish a successful result.

The refreshed `.appsdk/sdk-resources.json` uses `schema_version: 2`.
`version` retains the installed core Bundle version. `resource_version` records
the embedded Bundle version that supplied the refreshed `docs`, `rules`, and
`skills`. Contract entries remain tied to the retained installed manifest.
This separates resource supply from the core project pin; it does not upgrade
the core SDK, rewrite the lock, or relax ordinary core `verify` and `compile`
pin checks.

Resource refresh does not write `.appsdk/guidance/compiled.json`. After the
refresh, run the existing Guide compile command so the declared rule sources and
project Guidance machine contract are consumed again:

```bash
appsdk guide compile [project]
```

### Core version migration and pin-lock

Running AppSDK 0.1.6 `pin-lock` is the explicit supported migration from project
SDK 0.1.5 to 0.1.6, not a new-project initialization step. SDK canonical maps
and project governance maps are separate resources. Before changing live maps,
`pin-lock` classifies each map set: an exact 0.1.5 SDK canonical set is migrated
to the 0.1.6 canonical set; a custom project set is snapshotted and retained in
place. It validates frozen ReviewRecord bindings, persists one immutable
`.appsdk/migrations/0.1.5-to-0.1.6/` snapshot/record, then installs the 0.1.6
Bundle and writes the lock and project version. It must never overwrite custom
project maps or hand-edit ReviewRecord hashes. Missing, mixed, drifted, or
ambiguous migration truth fails closed.

For a later bundle refresh, a historical custom map's canonical target may
differ from the current SDK manifest only when the lock witnesses the original
bundle. The historical record and snapshot remain immutable, and each live
custom map must still match its recorded target digest. A repeated pin uses the
same previous-bundle witness; missing witnesses, malformed digests, altered
snapshots and changed live maps fail explicitly.

Resource-only refresh (`appsdk resources refresh`) does not perform core version
upgrades or rewrite `sdk.lock`. Core CLI commands (`new`, `init`, `compile`,
`pin-lock`) retain existing version and schema compatibility checks.

## Optional black-box test governance

AppSDK is the only owner of optional black-box test governance selection,
scope confirmation, scenario contracts, trusted runner references, effect
authorization, result evidence binding and final object admission. Missing or
`mode: "off"` test governance is compatible with the current project schema
and does not make `compile` or ordinary `verify` depend on a test manifest.
When `mode` is `selected`, the project contract requires a committed manifest
path under `contracts/test-governance.schema.json`, and selected scenario
results live under
`.appsdk/records/test-scenario-results/<object_id>/<scenario_id>.json`.

`appsdk verify --test-admission` lists selected object status without executing
tests. `appsdk verify --admission` applies the selected-object test-admission
gate; ordinary `appsdk verify` reports `not_selected`, `passed`, or `blocked`
without requiring tests to pass. `compile` intentionally does not load the
optional test manifest. Governance records and result records never include
executable shell strings; scenarios are bound only to trusted `runner_ref`
entries. DAGpipe CLI remains graph-only and is not a substitute for AppSDK test
evidence or admission. Existing module blackbox gates are unchanged.

## Runtime boundary

Runtime may consume only the compiled manifest and verified Active artifact. Runtime must not scan `.appsdk-control/`, Playground, Protected source, or arbitrary instruction files to reconstruct capability.

## Development Process Control Harness

The Harness is an agent-facing governance control plane, not a business runtime
capability. `.appsdk/project.json#/guidance/rule_sources` explicitly declares
project AGENTS, Skills, and their machine JSON contracts. `appsdk guide compile`
reads only those paths and writes deterministic `.appsdk/guidance/compiled.json`.
Markdown supplies declared context for the agent. Guide consumes declared paths
and machine contracts without ordinary source-identity digests; it validates
JSON nodes, edges, severity, and evidence contracts.

Active PlanRecord and append-only PlanRevisionRecord/StepExecutionRecord events
live under ignored `.appsdk-control/guidance/<task-id>/`. Existing AppSDK records
remain the lifecycle truth. Missing Harness state does not block ordinary
`verify` or `compile`; projects opt in and default to advisory guidance.

After `new`, or `init` on a project with an approved Guidance declaration,
follow the printed onboarding route:

```bash
appsdk guide compile <project>
appsdk guide init <project> --task <task-id> --mode <develop|debug> --module <module-id>
```

The read-only intake names declared AGENTS/local Skill sources, questions still
requiring user confirmation, exact Skill invocation suggestions, and the next
guide commands. It never scans undeclared directories or writes an intake
record. The Agent asks only unresolved questions and persists the confirmed
technical plan through `appsdk guide plan`.

If an existing governed project has no Guide declaration, rerun `appsdk init`
idempotently. It preserves existing project/maps/records/Active/Protected truth,
installs missing Guide resources, and prints:

```bash
appsdk guide init <project> --task guidance-setup --mode bootstrap --module <module-id>
```

The Agent reads the returned root AGENTS, bundled AppSDK Skill, and direct
project-local Skill candidates, then proposes reusable workflows, project
commands, evidence, severity, and source ownership. No durable rule is written
or compiled before explicit user approval. After approval, update AGENTS, the
project-local Skill and machine contract, and
`.appsdk/project.json#/guidance/rule_sources` in a clean owner worktree; then run
`appsdk guide compile` and `appsdk verify`. This setup proposal is project-level
and must not be replaced by a task PlanProposal.

`appsdk init` also attempts one initial Collab bootstrap for a live Codex App
Server Agent by invoking `collab init` once with the current environment.
Without a live Codex App Server session binding, AppSDK prints an explicit
Collab-pending result; it does not fabricate identity or subscription state.
Independent AppSDK work continues; Collab availability is decoupled from AppSDK
governance checks.

Frozen artifacts must be reproducible across clean worktree locations. The canonical module build runner appends a Rust `--remap-path-prefix` from the current project root to a stable logical path while preserving existing `RUSTFLAGS`. This removes absolute checkout paths from compiler metadata without changing source, payload, or lifecycle hashes. `rehydrate-frozen` and normal module compilation use the same runner; a path-dependent artifact is rejected rather than reconciled by copying or editing a frozen hash.

## Bootstrap

### New project initialization

1. Create and confirm preparation declaration:
   ```bash
   appsdk prepare ./new-project
   ```
   AI uses the `.appsdk-prepare.json` template to confirm: project kind (`new_project`, `module_refactor`, `project_refactor`, or `debug`), project root, legacy roots, Protected and forbidden boundaries. The operator confirms by setting `status: "confirmed"`. `init` rejects unconfirmed preparation.

2. Initialize governance skeleton:
   ```bash
   appsdk init ./new-project
   ```
   Or create from a clean template:
   ```bash
   appsdk new ./new-project
   ```

3. Verify governance integrity:
   ```bash
   appsdk verify ./new-project
   ```

Initialization automatically installs `.appsdk/docs/`, `.appsdk/rules/`, and `.appsdk/skills/`. These are governance inputs, not runtime capabilities; runtime consumes only compiled manifests and verified Active libraries.

Then fill and confirm `.appsdk/goal.json`, bind project maps and module ownership, and follow the promotion contract. Template creation requires an empty, non-symlinked destination.

### Existing project upgrade and governance ablation

- **Rule & skill refresh**: Use `appsdk resources refresh [project]` to update SDK-managed documentation, rules, and skills to the latest bundle resources without touching contracts, locks, or project state.
- **Legacy governance ablation**: Follow `appsdk-project-governance` Skill and [bootstrap-migration.md](../../sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md#existing-project-governance-ablation) to remove legacy SHA verification, identity candidate manifests, and `produce-lifecycle-chain` calls from project CI, hooks, and documentation.
- **Idempotent resource backfill**: Existing projects with `.appsdk/project.json` may rerun `appsdk init` to backfill newly added bundle resources without overwriting existing contracts or records.
- **Authorized legacy discard reset**: If historical migration witnesses or stale control-plane records block `verify` and the user explicitly authorizes discarding legacy baggage, use the canonical reset entry:
  ```bash
  appsdk init ./existing-project --fresh --discard-legacy
  ```
  This command requires a clean, non-main worktree. It removes old `.appsdk/` control state, carries forward valid `.appsdk/project.json` semantics into current SDK schema, preserves business code/active/protected files, and records `.appsdk/records/reset-governance-record.json` (`mode: "fresh_init"`). It is never run silently or as part of routine rule upgrades.

Review is an independent gate performed once at the major deliverable milestone boundary after author verification and real entrypoint regression pass.

## Lifecycle and development workflow

For ordinary development, bug fixing, and refactoring, follow the effective
project AGENTS and the host workflow. Global owners are `~/.agents/AGENTS.md`
and `~/.agents/skills/coding-principals/SKILL.md`. This document does not define
a second sequence or another admission gate.

Each requirement has a unique owner module; design choices remain within agreed boundaries; and acceptance criteria bind to verification gates. Debugging records reproducible error evidence, conditions, and results.

Legacy core compilation retains its explicit state constraints: `init` leaves
a core project at `draft`, and an early core `compile` returns
`COMPILE_BLOCKED`. These core commands follow their existing promotion contract;
resource refresh and Guide consumption do not invoke that lifecycle.

The project contract is auditable and committed; the SDK implementation is external, while `sdk.lock` binds the project to the declared AppSDK version/schema and Bundle resources rather than a particular executable digest. Local control state is disposable and ignored.

### Pre-release and publication boundary

The ceremonies of candidate packaging, `produce-lifecycle-chain`, `PreReviewValidationRecord`, `PromotionRecord`, `Active` library promotion, and `FreezeRecord` are ablated from ordinary daily development. They belong strictly to external release preparation, formal publication, and immutable artifact archiving:

- **Historical & external release records**: AppSDK retains commands such as `produce-lifecycle-records`, `produce-lifecycle-chain`, and `verify --review-admission` for backward compatibility with existing lifecycle records and formal promotion gates. See [lifecycle-records.md](lifecycle-records.md) and [playground-active-promotion.md](playground-active-promotion.md).
- **Proportionate verification**: Ordinary development requires concrete failure modes, red tests, and black-box regression against real entrypoints. It does not mandate SHA recalculation, patch reconstruction, or duplicate identity wrappers before commit.
- **Milestone review**: An independent architecture review runs once at the completion boundary of a major deliverable milestone. Internal commits, edits, and routine node steps do not trigger reviews.
- **Release distribution**: Formal distribution and cross-platform installation packages follow [release-distribution.md](release-distribution.md).
