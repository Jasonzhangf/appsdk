# G9/T16 实现检查点与外置盘恢复交接

时间：2026-10-07T14:05:00Z；恢复观察：14:06:52Z。状态：`INCOMPLETE`，盘已恢复，继续验收中。完整需求锁的 G1–G8 仍未实现。本文件保存盘掉线节点的历史状态与可恢复证据，不成为需求、授权或产品代码真源；后续以候选 evidence 与独占 run notes 为准。

## 候选与资源 owner

- 仓库：`/Users/fanzhang/Documents/github/appsdk`。
- 候选：`/Volumes/Intel/playground/appsdk/authoritative-review-template-20261007`。
- 分支：`codex/authoritative-review-template-20261007`。
- 基线：`c3c0c8df79e69534fe30c92db61328824473d5c0`。
- Issue：`592e241`。
- 独占 run：`/Volumes/Intel/playground/appsdk/.worker-runs/authoritative-review-template-20261007`；先读该目录的 `notes.md` 再推进。
- 候选尚未 commit，尚无包含实现的 candidate SHA。不能把基线 SHA 称为已验实现 SHA。
- Owner：本目标编排者。worktree、build 与原始 worker 记录仍有用途，禁止清理。
- 其他任务的 `docs/collab.md` dirty 及 `collab-identity-shortest-path-20261007` worktree 必须保留。

## 已完成的增量

1. 使用新建 `codex exec --profile gcm` worker 产出 owner 审计、SDK 模板、公开黑盒红测和独立设计审查。W4 首次 DESIGN_FAIL，修正旧 PASS 循环依赖后 W4R DESIGN_PASS。多名实现 worker 没有交付实质产物，已取消并记录；parent 接管其实现范围。这些取消不能写为 worker 成功。
2. SDK 0.1.0011 增加自有 `authoritative-review-template.md`，沿正式 bundle 分发，不依赖当前全局 reviewer 提示词。
3. 公开 `appsdk review-context [project] --module <id>` 从项目现有 goal、候选、scope、作者验证、证据与 SDK 模板生成 `{context_id, context, prompt}`。
4. 新架构 PASS 必须携 reviewer 明确返回的 `requirements_review: {context_id, checked: true}`。唯一持久 binding 位于 `ReviewRecord.project_bindings.requirements_review`，并在计算 review_id 前加入。
5. goal、候选、证据或模板变化后，现代旧 PASS 被拒绝；生成新 context 不依赖旧 PASS，可重新审查恢复。frozen/retired 保留历史语义。
6. 0.1.0010→0.1.0011 的版本、迁移、maps、schema、init 与 pin-lock 已在候选接线。
7. 两张 graph 注册为不可执行设计 operator；不是需求 runtime 实现。可执行增量是 AppSDK 公开 CLI。

消费设计 0.2.0 的六节点路径如下。正式图在候选 `docs/dagpipe/user-requirement-consumption.graph.json`；主树旧图不能覆盖候选新版。

```mermaid
flowchart LR
  A[加载当前需求] --> B[核验来源与版本]
  B --> C[绑定候选和 scope]
  C --> D[SDK 模板组装 review 材料]
  D --> E[独立 reviewer 核验与证据]
  E --> F[校验 binding 并发布准入结果]
```

本增量读取的 `.appsdk/goal.json` 仍不能证明用户身份认证或变更授权。context 中明确标出这些限制。`checked: true` 与哈希只表示显式确认和材料身份，不能证明实际模型遵循或用户独占写权限。测试 fixture 的确认是输入，不是用户认证证据。

## 作者验证：已观察结果

外置盘掉线前已经读取以下日志和工具回执；原始文件目前暂不可访问，恢复后复用有效证据。

- Rust 全部 targets 合计 519 例 PASS。`rust-full-attempt4.log` 中前八个 targets 共 517 例 PASS；最后 sdk_0009 的当前版本旧断言令整条 cargo 命令 exit 101。修正该 fixture 后，`sdk-migration-green.log` 的两个测试 PASS、exit 0。不得写成单次完整 cargo 命令 exit 0。
- CLI smoke 的 339 例包含公开上下文、合法确认、缺/假/陈旧确认、篡改、early-stage 旧 PASS 拒绝与公开 producer 恢复链。
- `dagpipe-tests.log` 的全 targets 测试 PASS，dagpipe runtime 源码未变。
- Rust format、JSON、source registry、`git diff --check` PASS。
- `release-build.log` 正式 release build exit 0。
- 当前消费 graph validate PASS；它证明拓扑，不证明 G1–G8 的实际效果。

## 安装与掉线：当前确认

官方 `scripts/install-global-appsdk.sh` 已返回 exit 0，原始日志位于 run 的 `install-global.log`。本次安装没有 daemon 重启动作。

随后外置 Intel 盘断开。读取候选失败，先出现 `Device not configured (os error 6)`，之后 exec 的 cwd 不存在。`diskutil info /Volumes/Intel` 返回 `Could not find disk: /Volumes/Intel`；`diskutil list external` 没有 Intel，只列出另一块 extension 外置盘。两条 Git worktree 登记因此显示暂时 prunable。这不是删除授权。

2026-10-07T14:05:00Z 的固定安装入口观察：

| 对象 | 结果 |
| --- | --- |
| `command -v appsdk` | `/Users/fanzhang/.cargo/bin/appsdk` |
| `appsdk version` | `appsdk 0.1.0011 (rust)` |
| 已安装 binary SHA-256 | `2d5b64065f1b985cc034043d2016908bf4be2cc9c052c7c5ba70a209de2bb59a` |
| 已安装 SDK 权威 review 模板 SHA-256 | `1f2acaea1a3a8f20e9e8bebdc2940d4d1db1d4594421eca389db61992a3fd1d4` |

这些值是产物观察，不是认证或完整需求锁证明。掉线阻断了 release/installed 字节来源核对、canonical CLI consumer 回放及后续 review。安装后行为验收尚未执行。未做实现后独立架构 review、commit、CI、merge、push 或最终资源回收。

## 恢复后继续

1. 确认原 Intel 盘恢复在原路径；读取候选与 run notes。禁止 prune、重建覆盖或用本地 worktree 回退。
2. 核对候选 dirty 的本任务归属、installer 原始回执、release/installed binary 和模板来源。复用已有效的测试证据。
3. 用 `APPSDK_TEST_BINARY=/Users/fanzhang/.cargo/bin/appsdk` 执行受影响公开 CLI consumer，覆盖 context、ack、stale PASS 与恢复。原记录和 build 位于本任务 run。
4. 将有效作者证据绑定实际 candidate 身份，再通过正式 Codex/AGY review 入口独立审查。无 PASS 不集成。
5. 刷新最新 origin/main，完成适用 CI、owner 正确的 clean 集成、push 回执及自有资源回收。不得覆盖主树他人的 `docs/collab.md`。

14:06:52Z 恢复观察：原 Intel 候选与 run 已恢复可读；installer 原始日志确认 exit 0；release 与已安装 binary 的 SHA-256 同为上表值，模板 `cmp` exit 0。已继续 canonical CLI 验收。无需重复安装或等待用户恢复回复。

继续提示词：

```text
继续 AppSDK G9/T16 权威需求 review 增量。先读 docs/evidence/user-requirement-truth-lock-20261007/implementation-checkpoint.md；恢复并读取原 Intel 候选的 run notes。沿原候选推进，不重建覆盖，不清理 prunable worktree。使用新建 codex exec --profile gcm worker，完成已安装 0.1.0011 的公开 CLI 验收、精确候选绑定、独立 review、CI、集成和自有资源收口。完整需求锁 G1–G8 仍是 gap，不得宣称用户独占修改已实现。外置盘仍不可用时保持 INCOMPLETE，不在本机另建项目 worktree。
```
