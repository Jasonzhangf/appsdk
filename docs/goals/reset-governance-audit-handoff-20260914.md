# AppSDK reset-governance audit and worker handoff

## 任务边界

本轮只修改 AppSDK。RouteCodex、Codex Desktop、Codex TUI、codexapp、V3、Collab
仓库和其他项目都不是本轮写入范围。Collab 不是 AppSDK 治理或开发的前置条件；如需
并发执行，worker 只能通过 Luna 规则启动 `codex exec --profile gcm`，不要调用
Collab，也不要把 `appsdk subagent` 当成 Desktop 原生 task/thread。

原始 AppSDK 主树必须保持原样：

```text
/Users/fanzhang/Documents/github/appsdk
branch: chore/project-memory-snapshot
HEAD:   8bc5c5d
dirty:  ?? docs/collab.md
```

它是 dirty 且由其他工作保留，禁止在这里开发、reset、restore、stash 或清理。

## 当前基线和交接 worktree

集成工作树：

```text
/Users/fanzhang/Documents/github/appsdk-bug-integration
branch: main
HEAD:   b49456bb849be623ea2f5e9a4f6060802995f872
origin/main: b49456bb849be623ea2f5e9a4f6060802995f872
status: clean（交接文档写入前）
```

本次实现唯一 worktree：

```text
/Users/fanzhang/Documents/github/appsdk-bug-integration/playground/reset-governance-transactional-20260914
branch: codex/reset-governance-transactional-20260914
base:   b49456b
owner:  本 handoff 后的新 GCM worker
```

旧的被停止 worker 只读了代码，没有提交实现。写入本文件后该 worktree 会变 dirty；
新 worker 接手后必须把文档和实现一起纳入自己的提交，或明确回报文档保留在哪个已提交
commit 中。

## 已完成的 AppSDK 变更

`integration/main` 已包含并推送以下修复：

```text
40a9041  init: reset stale lifecycle evidence
7329947  verify: skip discarded artifact in reset epoch
8823507  communication: distinguish missing global registry
b49456b  communication: require host execution for terminal receipts
```

当前已验证：

- `communication_cli`: 89/89 通过；
- `cli_smoke`: 219/219 通过；
- `cargo fmt --manifest-path rust/Cargo.toml -- --check` 通过；
- `cargo check --all-targets --manifest-path rust/Cargo.toml` 通过；
- release build 通过；
- 独立 GCM review 两轮均为 PASS，P0/P1 为 0；
- 官方安装入口已执行：`scripts/install-global-appsdk.sh`；
- 当前全局 binary 是 `appsdk 0.1.6 (rust)`，路径 `/Users/fanzhang/.cargo/bin/appsdk`，
  SHA-256 `f61273061b5ff6c8e55f47ff79375f9c95f6715b174461c85967bca4b0ca1e31`。

这些证据对应 `b49456b`，不等于本轮 reset-governance 修复已经实现、review、merge、
重新安装或关闭 bug。安装、daemon 重启和 bug close 必须在新提交通过黑盒验证后分别记录。

## 审计报告的有效结论

审计报告指出 `rust/src/main.rs` 的 `reset_governance_internal` 保留了两条语义不同的
reset 路径：

1. `init --fresh --discard-legacy` 获取 reset 事务锁，恢复未完成事务，构建 staging，
   校验并保留现有 `.appsdk/project.json`，再 quarantine/publish/rollback。
2. `reset-governance --discard-legacy` 在发现固定路径
   `.appsdk/records/reset-governance-record.json` 时直接打印成功；否则直接删除
   `.appsdk`、`.appsdk-control` 和 generated roots，再调用 `write_project_scaffold`。

第二条路径会在删除 `.appsdk` 后生成默认项目合同，可能丢失 `project_id`、模块边界、
构建声明和其他项目拥有字段；删除过程没有 staging 和回滚。历史 reset record 还会被
误当成当前操作已经完成，无法开始新的治理周期。报告要求治理历史可以重新建立，质量
结论仍必须重新证明；失败应阻断危险交付，不应冻结开发。

对应当前源代码证据（以交接基线为准）：

```text
rust/src/main.rs:16269-16402  reset_governance_internal 与旧的直接删除分支
rust/src/main.rs:13341-13410  reset-staging-scaffold 与合同保留/校验
rust/src/main.rs:15817-16088  reset_transaction_fresh 的事务引擎
rust/tests/cli_smoke.rs:505-542  旧 reset-governance 测试未验证合同保留
```

已登记本轮缺陷：

```text
5c68429  [SDK Bug] reset-governance must reuse transactional fresh-init contract preservation
status   open, P1, cli/governance/init
```

已有的 `b6b7007`（AppServer terminal receipt 伪装）和 `35a4765`（global registry 缺失
分类）修复已经进入 `b49456b`，但要等本轮安装后的真实黑盒 receipt 才能关闭；不得提前
伪造 close receipt。

## 新 worker 的实现目标

优先复用已有事务引擎，避免再造 reset 实现：

1. 让 `reset-governance --discard-legacy` 和 `init --fresh --discard-legacy` 进入同一
   transactional reset owner；两入口只保留必要的 mode/输出差异。
2. 两入口都获取稳定 reset lock，并先执行 `reset_transaction_recover`。已提交但尚未
   清理的事务应幂等收口；未提交事务应回滚并返回明确的重试信号。
3. 删除“只要 reset record 存在就成功”的短路。每次新的 CLI reset 生成新的
   `transaction_id`；同一未完成事务只按 marker 恢复一次。若 record schema 只允许一个
   当前 record，不要未经合同设计擅自复制历史真相；把这个边界写入测试或剩余风险。
4. 通过 `reset-staging-scaffold` 读取并验证现有项目合同，按现有
   `normalize_fresh_project_contract` 规则只升级受支持的 SDK pin，保留所有其他项目字段。
5. 只移除控制面、声明的 generated roots 和可重建的 SDK contract projection；保留业务源、
   runtime data、`active/`、`protected/`。保留现有 symlink/path guard、非 `main` 和 clean
   worktree 约束。
6. 为非 fresh mode 写入合法 reset record；让 committed recovery 能验证 fresh 与
   discard 两种合法 mode，不能把错误包装成成功。
7. 增加有意义的 CLI 回归：非 fresh reset 保留自定义 `project_id`/模块字段；第一次和第二次
   reset 都真实执行且不复制旧 records；事务构建、path guard 或合同无效时不污染原状态；
   fresh 与 reset 的保留/删除集合一致。不要为了覆盖率复制实现细节测试。

## 新 worker 派单合同（可直接复制）

```text
你是 AppSDK reset-governance 修复的唯一实现者。使用 GCM worker，在已有独立 worktree
/Users/fanzhang/Documents/github/appsdk-bug-integration/playground/reset-governance-transactional-20260914
上工作；分支 codex/reset-governance-transactional-20260914，基线 b49456b。还有其他执行者，
不要覆盖、回滚或清理他们的 worktree；不要修改 /Users/fanzhang/Documents/github/appsdk，
尤其不要碰其 dirty docs/collab.md；不要修改 RouteCodex 或任何非 AppSDK 项目，不要运行
Collab。

目标：修复 bug 5c68429。审计发现 reset-governance --discard-legacy 与
init --fresh --discard-legacy 分叉：旧 reset 路径在已有 reset-governance-record.json 时
假成功，否则直接删除 .appsdk/.appsdk-control/generated 后用 write_project_scaffold，
可能丢失项目合同且不可回滚。

实现 iff：两入口复用同一 reset transactional owner；获取 lock、恢复 in-flight marker、
分阶段 staging/quarantine/publish/rollback；保留并验证现有 project.json（只按现有规则把
支持的旧 SDK pin 正规化）；保留业务源、runtime、active、protected；只移除控制面和声明
generated roots；移除历史 reset record 短路；fresh 与 discard mode 的合法 reset record
都能恢复；保留所有 symlink/path、非 main、clean worktree 和错误显式返回约束。优先复用
reset_transaction_fresh，不新增第二套引擎。

允许修改：该 worktree 内 rust/src/main.rs、rust/tests/cli_smoke.rs，以及为解释本次
合同而确有必要的 AppSDK docs。禁止修改其他路径。

测试条件（按顺序，全部通过才算完成）：
  cargo fmt --manifest-path rust/Cargo.toml
  cargo fmt --manifest-path rust/Cargo.toml -- --check
  cargo test --manifest-path rust/Cargo.toml --test cli_smoke --quiet
  cargo check --all-targets --manifest-path rust/Cargo.toml
  git diff --check

交付：只在测试全绿后提交一个 Conventional Commit；回报 commit SHA、改动文件、测试
摘要、reset-governance 与 fresh 的黑盒行为、未解决风险。不要声称已 merge、push、安装、
重启 daemon 或关闭 bug；这些由 owner 在 review 后单独执行。
```

## 交接和收口顺序

1. 新 worker 先读取本文件、`AGENTS.md` 和当前源代码，再在本 worktree 实现；不要依赖父
   transcript、resume、fork 或隐式环境。
2. worker 回传 SHA 后，owner 只读检查 diff、测试和 worktree 状态，并做独立 review；review
   必须与实现者分离。若失败，从首个失效 gate 重跑，不重复已经有效的通信/全量证据。
3. review PASS 后，在 `/Users/fanzhang/Documents/github/appsdk-bug-integration` 的
   `main` 上集成该 commit，确认 hook、`git status`、HEAD、`origin/main` 和 remote SHA
   receipt；不得把 worker branch 的本地状态当成 main 已集成。
4. 集成后重新执行受影响的 reset/CLI 验证和必要 release build，再运行唯一官方安装入口：

   ```text
   /Users/fanzhang/Documents/github/appsdk-bug-integration/scripts/install-global-appsdk.sh
   rehash
   appsdk version
   shasum -a 256 /Users/fanzhang/.cargo/bin/appsdk
   ```

5. 只有安装后的真实黑盒 receipt 具备时，才分别关闭 `5c68429`、`b6b7007`、`35a4765` 中
   已满足的项；任何失败保留原始错误和首次偏离。

## worktree 清理规则

交接期间保留当前实现 worktree，不得因为“看起来旧”删除。收口时先执行
`git worktree list --porcelain`，逐项确认 owner、branch、无运行 worker、无未提交改动和
已合入远端；只移除本任务实际创建且已完成的
`playground/reset-governance-transactional-20260914`。使用普通
`git worktree remove <path>`，不要 `--force`、不要手删目录、不要删除原始 dirty 主树或
其他 worker 的 worktree/branch。若 worker 留有未合并改动，先回到该 owner 处理，不能以清理
代替集成。

清理证据与工程证据分开报告：worktree removed 不代表 commit merged、remote pushed、
binary installed、daemon restarted 或 bug closed。

## Worker 实现记录

`reset_governance_internal` 已把 `init --fresh --discard-legacy` 与
`reset-governance --discard-legacy` 收口到同一 transactional reset owner：两入口都用
`ResetMode` 参数进入 `reset_transaction_run`，共用 reset lock、`reset_transaction_recover`、
staging → quarantine → publish → rollback、`reset-staging-scaffold` 合同保留，以及
`normalize_fresh_project_contract` 的受支持 SDK pin 正规化。旧 reset 的
`reset-governance-record.json` 存在即成功短路已删除；每次新 operation 用
`reset_transaction_id(mode)` 生成新 `transaction_id`。marker、committed recovery 与 reset
record 现在都绑定 mode；`fresh_init` 与 `discard_legacy_control_plane` 两种合法 mode 都可
幂等收口。两入口删除/保留集合一致：只移除 `.appsdk`、`.appsdk-control`、声明的 generated
roots 与可重建 SDK contract projection，保留业务源、runtime data、`active/`、`protected/`
和现有 `.appsdk/project.json` 项目字段。

回归验证（`rust/tests/cli_smoke.rs`）：

- `reset_governance_preserves_contract_and_starts_a_new_transaction`：普通 reset 保留自定义
  `project_id`、模块字段与 build 声明；移除旧控制面、legacy record、generated projection 与
  第二次写入的 `.appsdk` 内容；历史 reset record 不再造成假成功；第二次 reset 生成新的
  `transaction_id`；reset lock 在两次操作后都已释放。
- `reset_governance_recovers_committed_discard_transaction_marker`：`discard_legacy_control_plane`
  的 committed transaction 幂等收口；对应 fresh 版本既有
  `init_fresh_committed_cleanup_uses_marker_roots_after_legacy_contract_is_gone` 继续通过。
- `reset_receipt_validation_is_mode_aware_and_fail_closed`：两种 mode 都要求
  `transaction_id` 与 `reset_id` 一致，缺失即 `INVALID_RESET_GOVERNANCE_RECORD`。

门禁结果：`cargo fmt`（含 `--check`）、`cargo test --test cli_smoke`（220/220）、
`cargo check --all-targets`、`git diff --check` 全部通过。

未验证黑盒边界：本 worker 只做本地源码与 CLI 测试，未安装 binary、未重启 daemon、未做
真实项目黑盒 replay；未覆盖安装后的真实 reset receipt。
