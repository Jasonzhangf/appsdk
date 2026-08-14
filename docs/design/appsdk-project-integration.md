# AppSDK Project Integration

AppSDK is an external governance implementation. A new project consumes its CLI/compiler and commits only project-specific governance data.

## Repository boundary

```text
external AppSDK installation
  -> CLI / compiler / contracts / harness / adapters
  -> .appsdk project contract + sdk.lock
  -> compiled manifest and verified artifact
  -> project runtime consumes Active
```

The business project must not copy AppSDK source, compiler, or harness into its own source tree.

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

## SDK lock

`.appsdk/sdk.lock` binds the project to the external implementation:

```json
{
  "sdk": "appsdk",
  "version": "0.1.0",
  "digest": "sha256:replace-with-compiled-sdk-digest",
  "compiler_digest": "sha256:replace-with-compiler-digest",
  "binary_ref": "project-sdk",
  "contract_schema": 1
}
```

The lock is committed. A template may retain the two documented `replace-with-*` values while the project is `draft`; compile, promotion, and freeze reject those values with `SDK_LOCK_NOT_PINNED`. The external SDK is replaceable only through an explicit versioned migration; runtime does not scan or infer a different SDK.

## Runtime boundary

Runtime may consume only the compiled manifest and verified Active artifact. Runtime must not scan `.appsdk-control/`, Playground, Protected source, or arbitrary instruction files to reconstruct capability.

## Bootstrap

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

Then fill and confirm `.appsdk/goal.json`, bind project maps and module ownership, and follow the promotion contract. Template creation requires an empty, non-symlinked destination.

## Lifecycle

```text
goal clarification
  -> confirmed/admitted
  -> Playground experiment
  -> EvidenceRecord
  -> ReviewRecord PASS
  -> candidate artifact
  -> PromotionRecord
  -> Active library
  -> Protected source/contracts
  -> FreezeRecord
```

The project contract is auditable and committed; the SDK implementation is external and pinned; local control state is disposable and ignored.
