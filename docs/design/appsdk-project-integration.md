# AppSDK Project Integration

AppSDK is an external governance implementation. A new project consumes its CLI/compiler and commits only project-specific governance data.

## Repository boundary

```text
external AppSDK installation
  -> versioned Bundle: CLI / compiler / contracts / docs / rules / skills
  -> .appsdk project contract + sdk.lock
  -> compiled manifest and verified artifact
  -> project runtime consumes Active
```

The business project must not copy AppSDK source, compiler, or harness into its own source tree.

## Governance preflight ownership

Governance readiness is part of the implementation task. Missing contracts,
maps, records, SDK locks, clean worktrees, claims, adapters, or required
evidence are owned project-management defects. The worker stops business-code
debugging, repairs the first failing governance node through its canonical
owner, and reruns preflight before continuing. External tooling or network
failures may describe the immediate symptom, but never justify skipping,
weakening, or bypassing the governance lifecycle.

## Collaboration operating contract

Lifecycle mutations run only from a clean named branch in a worktree below the
main worktree's `playground/` directory. The main worktree is an inspection
surface, never a development or lifecycle-write surface. The worker fetches
latest `origin/main`, creates the worktree, reproduces, modifies, builds,
commits, pushes, integrates the exact candidate with latest main, verifies the
integration, fast-forwards remote main, validates it with `git ls-remote`, and
then cleans only the merged worktree and branch. AppSDK rejects mutation
commands on branch `main`; `verify` remains read-only.

Existing projects retain valid historical governance. Version migration
preserves immutable historical evidence, but unified AppSDK + dispersed Collab
upgrade is an authorized ownership-transfer operation. The two control planes
are inspected and snapshotted independently, mapped explicitly, and reconciled
only after Jason authorizes control of the named objects. Authorized cleanup
may remove proven-stale staging and disposable runtime projections; it must not
delete audit history or referenced immutable artifacts. After reconciliation,
only authorized runtime/temporary state is reset to zero and the new unified
truth is verified from a clean initial state. Historical constraints that are
stricter than the target version are warnings during transition; integrity,
ownership, isolation, hash, Protected/Active immutability, ambiguous mapping,
duplicate writers, unauthorized cleanup, and remote-main receipt failures
remain forbidden.

The migration contract is:

```text
inspect AppSDK truth + every Collab initialization root
  -> freeze and independently snapshot both planes
  -> map exact matches, one-sided legacy state, and conflicts
  -> design the complete compliant reconciliation route
  -> request approval for the exact ownership-transfer/destructive step
  -> execute the approved route; keep unapproved destructive steps frozen
  -> clean only authorized stale disposable state
  -> install/pin new AppSDK and run official Collab v1 migration
  -> verify one owner/one truth, then zero runtime state
```

Do not directly merge `.agent-collab` directories or hand-edit task, claim,
mailbox, identity, migration, hash, Active, or Protected records. A Collab-only
object without an authorized owner receives a proposed disposition and mapping
route; it is not automatically adopted or deleted. A conflict requires the
worker to provide the target truth, exact object operations, rollback boundary,
and verification gates before requesting approval. Approval is an execution
gate, not a report-and-wait terminal state.

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

## Git policy

Commit:

- `.appsdk/project.json`;
- `.appsdk/goal.json`;
- `.appsdk/sdk.lock`;
- project resource/function/mainline/verification maps;
- Evidence, Review, Promotion, and Freeze records;
- project architecture and verification contracts;
- Active and Protected content when the project uses Git as its artifact/archive store.

Ignore:

```gitignore
.appsdk-control/
```

`.appsdk-control/` may contain local review sessions, worker heartbeat, temporary harness output, and caches. It must not contain project truth, immutable rules, SDK lock, maps, or lifecycle records.

## SDK Bundle and lock

消费者只使用全局安装的 AppSDK Bundle。Bundle 版本必须同时绑定 Rust CLI、machine contracts、文档、规则和 skills；不能让项目自行扫描一个不受版本控制的全局目录。Bundle manifest 是机器真源，`init`/`new` 将其中声明的 Skill、规则和文档安装到项目 `.appsdk/`，并生成 `.appsdk/sdk-resources.json`。

项目中的手动合同（例如 `project.json`、records、maps）由 AI/开发者维护，SDK 只校验 schema、引用、owner、scope 和生命周期关系。`.appsdk/sdk-resources.json` 是自动生成文件，只能由 SDK 重建；`verify` 会校验资源文件摘要和 Bundle digest，发现手工改写立即 fail-fast。

`contracts/sdk-bundle.manifest.json` 声明当前 Bundle 的资源集合和安装位置；初始化后项目内的机器真源位于 `.appsdk/contracts/sdk-bundle.manifest.json`。它和 binary 同版本发布；`pin-lock` 把 binary digest、Bundle digest、manifest digest 和资源集合一起写入锁。执行 `pin-lock` 的 binary 必须与 `--binary` 指向的文件字节一致，防止旧 CLI 把新 binary 与旧 embedded Bundle 拼成伪锁。

`.appsdk/sdk.lock` binds the project to the external implementation:

```json
{
  "sdk": "appsdk",
  "version": "0.1.6",
  "digest": "sha256:replace-with-compiled-sdk-digest",
  "compiler_digest": "sha256:replace-with-compiler-digest",
  "bundle_digest": "sha256:replace-with-sdk-bundle-digest",
  "bundle_manifest_digest": "sha256:replace-with-bundle-manifest-digest",
  "bundle_resources": {"contracts": [], "docs": [], "rules": [], "skills": []},
  "binary_ref": "project-sdk",
  "contract_schema": 1
}
```

The lock is committed. A template may retain the two documented `replace-with-*` values while the project is `draft`; compile, promotion, and freeze reject those values with `SDK_LOCK_NOT_PINNED`. Running AppSDK 0.1.6 `pin-lock` is the explicit supported migration from project SDK 0.1.5 to 0.1.6. SDK canonical maps and project governance maps are separate resources. Before changing live maps, `pin-lock` classifies each map set: an exact 0.1.5 SDK canonical set is migrated to the 0.1.6 canonical set; a custom project set is snapshotted and retained in place. It validates the frozen ReviewRecord bindings, persists one immutable `.appsdk/migrations/0.1.5-to-0.1.6/` snapshot/record, then installs the 0.1.6 Bundle and writes the lock and project version. It must never overwrite custom project maps or hand-edit ReviewRecord hashes. A partially completed 0.1.6 pin may resume only through that exact migration record or an exact 0.1.5 source map set. Historical frozen reviews resolve their old hashes only through this snapshot; current reviews must bind live maps. Missing, mixed, drifted, or ambiguous migration truth fails closed. Other source versions fail with `UNSUPPORTED_SDK_MIGRATION`; runtime does not scan or infer a different SDK.

## Runtime boundary

Runtime may consume only the compiled manifest and verified Active artifact. Runtime must not scan `.appsdk-control/`, Playground, Protected source, or arbitrary instruction files to reconstruct capability.

## Development Process Control Harness

The Harness is an agent-facing governance control plane, not a business runtime
capability. `.appsdk/project.json#/guidance/rule_sources` explicitly declares
project AGENTS, Skills, and their machine JSON contracts. `appsdk guide compile`
reads only those paths and writes deterministic `.appsdk/guidance/compiled.json`.
Markdown is digest-bound context for the agent; AppSDK validates only declared
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

Frozen artifacts must be reproducible across clean worktree locations. The canonical module build runner appends a Rust `--remap-path-prefix` from the current project root to a stable logical path while preserving existing `RUSTFLAGS`. This removes absolute checkout paths from compiler metadata without changing source, payload, or lifecycle hashes. `rehydrate-frozen` and normal module compilation use the same runner; a path-dependent artifact is rejected rather than reconciled by copying or editing a frozen hash.

## Bootstrap

初始化前先创建需求准备文档：

```bash
appsdk prepare ./existing-workspace
```

AI 使用 `.appsdk-prepare.json` 模板向用户确认：这是新项目、模块重构、项目重构还是 debug；新项目根目录是什么；旧代码和 V3 等 legacy roots 是什么；新代码、Protected 和禁止修改边界是什么。用户确认后将 `status` 更新为 `confirmed`，然后才能执行 `init`。没有确认记录时，`init` 会 fail-fast。

已经存在 `.appsdk/project.json` 的治理根可以直接幂等重跑 `appsdk init`，用于补齐
当前 SDK 的 Guide 等 bundle 资源。此路径以现有项目合同作为 root 真源，不要求重新
创建 preparation record，也不覆盖项目合同或 lifecycle truth。新建、迁移到新 root、
或通过 `--project-root` 创建尚不存在的治理根仍必须走 confirmed preparation。

对已有工作区，confirmed preparation 后必须先进入旧状态迁移预检：

```text
confirmed preparation
  -> inspect 旧 AppSDK truth + 每个分散 Collab 初始化根
  -> 冻结并分别生成快照
  -> 设计控制权转移/冲突消解路线
  -> 请求精确接管、清理、清零动作的授权
  -> 执行已批准迁移并验证统一运行态归零
  -> appsdk init ./existing-workspace --project-root new-code
```

这是初始化流程的一部分，不是初始化后的可选升级。没有 legacy root
的新空项目只跳过 legacy inspection 分支；已有项目不能用 `init` 隐藏未解决的旧状态或冲突。

已有项目先执行：

```bash
appsdk init ./existing-workspace --project-root new-code
```

`init` 的第一个参数是已有工作区，`--project-root` 是新 AppSDK 项目的相对根目录。这样旧代码可以留在工作区，新代码和治理面进入独立子目录。它是幂等操作：创建 `playground/`、`active/lib/`、`protected/`、`generated/`、`.appsdk/` 和 `.appsdk-control/`，只补齐缺失的治理合同，并向新项目根目录的 `.gitignore` 追加一次 SDK 管理区块。它不覆盖已有项目文件，也不覆盖已有 Git 忽略规则；绝对路径和 `..` 路径会被拒绝。

新项目执行：

```bash
appsdk new ./my-app
appsdk verify ./my-app
```

初始化会自动安装 `.appsdk/docs/`、`.appsdk/rules/` 和 `.appsdk/skills/`。这些是项目治理输入，不是 runtime capability；runtime 只消费编译后的 manifest 和 Active library。

Then fill and confirm `.appsdk/goal.json`, bind project maps and module ownership, and follow the promotion contract. Template creation requires an empty, non-symlinked destination.

Review 是必需生命周期门禁，但工具不固定。若 Jason 指定 review 工具，且该工具提供只读、可观测、结构化 verdict，则必须使用指定工具；未指定时使用配置的默认 review 路由。记录工具、exact commit、scope、verdict 和 evidence。指定工具不可用时不得静默替换。

### Admission adapter contract and deterministic recovery

Admission is a read-only gate. It never manufactures a lifecycle record from a
missing file. When a required record is absent, the command emits a structured
`REVIEW_ADMISSION_BLOCKED` object containing the module, every missing and
present producer input, the single next action, `retry_allowed: false`, and the
forbidden shortcuts. Repeating the command before that external state changes
is idempotent and must produce the same blocked state; agents must not poll or
retry it.

The project owns adapters that observe reality; AppSDK owns record schema,
identity binding, causal ordering, and final admission. The supported sequence
is:

```text
clean owner worktree + candidate commit
  -> lifecycle adapter binds FixCandidateRecord
  -> whitebox adapter runs and records the actual whitebox result
  -> deployment adapter installs and restarts the exact artifact, recording both receipts
  -> blackbox adapter exercises the deployed public entrypoint
  -> lifecycle adapter binds PreReviewValidationRecord
  -> appsdk verify --review-admission <project> --module <id>
```

Each adapter is rerunnable for the same candidate without changing a PASS
record. A changed candidate, artifact, environment, entrypoint, or input starts
a new evidence set and invalidates the old one. No adapter may accept a
hand-entered hash, relabel whitebox output as blackbox output, or use an
artifact from another worktree/project/version.

For an upgrade, run `appsdk prepare`/`init` idempotently, inspect and snapshot
the old project and legacy roots, obtain explicit ownership-transfer approval,
run the pinned target binary's migration command, then execute the adapter
sequence above. Only after admission passes may review, merge, install,
restart, promotion, and freeze proceed. A blocked adapter is an actionable
external capability gap, not permission to edit records manually.

`init` intentionally leaves a project at `draft`. The required promotion before
compilation is explicit: confirm the goal, promote the project to
`source_implemented`, bind the module contract, promote the project to
`contract_bound`, and only then run `compile`. If compilation is attempted
early, AppSDK returns `COMPILE_BLOCKED` with this exact ordered continuation and
`retry_allowed: false`; it does not create a partial module artifact.

## Lifecycle

新 feature 和新项目在进入代码实现前必须完成闭环设计：

```text
需求与验收标准
  -> goal/scope/non-goals 确认
  -> 自上而下 module/resource map
  -> 概要设计
  -> 详细设计与调用/数据流绑定
  -> requirements -> design -> module -> verification 一致性检查
  -> 干净 worktree 开发
```

每个需求必须有唯一 owner module，每个设计元素必须落在允许边界内，
每个验收标准必须有 verification gate。设计缺失或矛盾是 worker 自身的
治理前置失败，不得先写业务代码。

Debug 必须保存思维链、错误证据、实验条件与结果、根因判断；合并前必须
对集成后的 tree 重新做 resource/function/mainline/module map、边界、
payload/control 隔离、owner 唯一和重复实现架构检查。

```text
goal clarification
  -> confirmed/admitted
  -> one semantic claim + one clean isolated worktree per worker
  -> baseline reproduction
  -> committed fix candidate + positive/negative evidence
  -> development whitebox PASS
  -> build + install + restart
  -> deployed public-entrypoint blackbox PASS
  -> PreReviewValidationRecord + `appsdk verify --review-admission` PASS
  -> architecture boundary check
  -> selected review tool ReviewRecord PASS
  -> unchanged-source effectiveness replay PASS
  -> one independently verifiable milestone per clean worktree
  -> commit + serial merge queue + tested integration for every milestone
  -> start the next milestone only after the predecessor remote-main receipt
  -> verified local and remote mainline receipt
  -> PromotionRecord
  -> Active library
  -> Protected source/contracts
  -> FreezeRecord
```

The project contract is auditable and committed; the SDK implementation is external and pinned; local control state is disposable and ignored.
