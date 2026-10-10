C 范围已完成：实现、测试变更、静态自检、notes/report 和三份 patch 均已落盘。未运行 Cargo，未提交、安装或触碰版本/迁移。最终状态如下。

**交付**

- [init.rs](/Users/fanzhang/Documents/github/appsdk/rust/src/main/init.rs:82)：ordinary init 在首次写入前读取并校验现有 project pin；旧 pin 返回 typed `SDK_VERSION_MIGRATION_REQUIRED` 并指向 `pin-lock`。`write_current_sdk_lock` 不再静默跳过不匹配 pin。`--fresh` 路径不变。
- [intake.rs](/Users/fanzhang/Documents/github/appsdk/rust/src/guidance/intake.rs:274)：proposal 的 readiness、questions、schema、`agent_instruction`、`after_user_approval`、`next` 统一为条件授权语义：先读上层规则/本地 Skills/测试/CI hook，差异审计字段含 path/owner/action/basis/保留保障/入口，复用既有会话授权，只对未覆盖 durable 差异请求批准，Guidance 未选用时不强制 compile。`approval_required` 由布尔改为 `uncovered_durable_changes_only`。
- [part_07.rs](/Users/fanzhang/Documents/github/appsdk/rust/tests/cli_smoke/part_07.rs:1424)、[part_13.rs](/Users/fanzhang/Documents/github/appsdk/rust/tests/cli_smoke/part_13.rs:935)：新增公开 CLI 红→绿用例，复用正式 `0.1.0011` archive，覆盖旧 pin 写前无副作用、官方迁移后 init 成功且保留项目自有文件、重复 init 幂等、proposal 只读条件授权字段。

**验证**

`rustfmt --check` 和 `git diff --check` 通过。未运行 Cargo，按合同由 controller 在组合树执行红→绿验收。

**交接物**

- [tests-only.diff](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/init-pin/tests-only.diff)，SHA `0f053d...`
- [implementation.diff](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/init-pin/implementation.diff)，SHA `0afdbf...`
- [patch.diff](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/init-pin/patch.diff)，SHA `9f9507...`
- [notes.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/init-pin/notes.md) 与 [report.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/init-pin/report.md)

controller 先应用 tests-only 跑旧逻辑红，再应用 implementation 跑绿。版本、迁移、maps 和 release 仍属 integration owner E。