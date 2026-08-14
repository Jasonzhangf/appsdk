# AppSDK Rust Binary Governance Completion Plan

## 目标

把当前 AppSDK 治理框架完成为可发布、可复现、可接入新项目的 Rust 二进制 + 文档包；业务项目只保留 `.appsdk/` 项目合同、records、maps 和 `sdk.lock`，不携带 AppSDK 源码或内部 harness。

## 验收标准

- Rust CLI 覆盖 `version`、`new`、`verify`、`compile`、`promote`、`promote-module`、`freeze`、`publish-active`。
- Rust 实现成为唯一正式交付真源；仓库不保留第二套治理实现或旧运行时入口。
- Rust 行为覆盖 goal confirmation、SDK lock、生命周期、zone、module ownership、artifact hash、record graph、Active/Protected freeze。
- 完整 module contract 进入 artifact/hash，冻结后 owner、owned paths、Active surface、Generated surface 变更会失败。
- Promotion、Freeze 使用候选 artifact 进行 evidence/review/hash 一致性校验。
- SDK 可构建 release binary，发布 digest 可写入项目 `.appsdk/sdk.lock`。
- 文档包、contracts、templates、Skill 与 binary 使用同一 release version。
- 新项目可通过外部 binary + 文档完成 bootstrap，不复制 AppSDK 实现，不污染业务源码。
- 完成 Rust CLI 黑盒测试、项目测试、Rust 测试、verify、Skill 校验和最终 architecture review PASS。

## 范围

包含：

- Rust lifecycle/promotion/freeze/record graph 迁移；
- Active publish、Protected archive、artifact registry 基础合同；
- SDK release、digest、lock 和文档包；
- 新项目黑盒接入验证；
- CI/build gates 和最终 review。

不包含：

- RouteCodex 业务语义迁移；
- provider、pipeline、metadata center 业务实现；
- 以 shell ACL 伪装成 Protected 源码不可读；
- fallback、双路径运行或长期双 ownership。

## 设计原则

```text
Rust binary + docs = external governance implementation
.appsdk/ = committed project governance truth
.appsdk-control/ = ignored local runtime state
Playground = mutable experiment source
Active = immutable consumable artifact
Protected = frozen history
Generated = deterministic compiler output
```

Rust binary 是唯一治理执行入口；contracts、templates、docs 和 Skill 必须与 binary 同 release version/digest。

## 技术方案与文件面

- `rust/src/main.rs`: Rust CLI 与治理主线；后续按模块拆分 lifecycle、records、artifact、publish。
- `rust/Cargo.toml`: binary release 依赖与版本。
- `contracts/`: machine-readable schema 和 transition source。
- `docs/design/rust-binary-delivery.md`: 二进制交付合同。
- `docs/design/appsdk-project-integration.md`: 新项目植入合同。
- `skills/appsdk-project-governance/`: 可复用 Skill 与 goal prompt reference。
- `tests/`: Rust/旧实现行为对齐与黑盒接入测试。
- `.appsdk/sdk.lock`: 外部 binary/compiler/docs release digest。

## 风险与规避

- Rust 行为回归：每个命令保留正/反 CLI 黑盒测试。
- artifact 与 project 半写入：候选 manifest 全量校验后原子写入并回读验证。
- SDK lock 伪锁定：release 阶段生成真实 digest，compile/promote/freeze 拒绝占位值。
- 旧实现复活：仓库禁止重新加入第二治理实现或旧运行时入口。
- 业务项目污染：黑盒检查项目不出现 SDK source/compiler/harness 副本。
- review 尚未 PASS：任何未明确 PASS 的 review 都不算完成。

## 验证计划

- `cargo fmt --check`、`cargo test`、`cargo build --release`；
- Rust `cargo test`、release build、CLI blackbox、verify；
- Rust CLI new/verify/compile/promotion/freeze 正反 smoke；
- record graph、freshness、scope、module、artifact hash 反测；
- SDK release digest 与 `.appsdk/sdk.lock` 对齐测试；
- 新项目黑盒：外部 binary bootstrap，确认无 SDK 源码污染；
- Skill validation、文档路径检查、CI gate；
- 最终 Codex architecture review 明确 `VERDICT: PASS`。

## 实施步骤

1. 修复当前冻结 module contract 完整 hash 绑定。
2. Rust 迁移 `compile`、artifact canonicalization、SDK lock。
3. Rust 迁移 `promote` 和 project/artifact 原子一致性。
4. Rust 迁移 `promote-module` 和 module scope evidence。
5. Rust 迁移 `freeze`、Protected archive、FreezeRecord。
6. Rust 迁移 Evidence/Review/Promotion record graph。
7. 实现 Active publish、version registry 和 immutable previous Active。
8. 实现 release binary/docs/contracts/templates/Skill digest。
9. 建立新项目黑盒接入和 CI/build gate。
10. 完成 Rust-only 验证，确认没有第二治理实现残留，完成最终 review。

## 完成定义

- 外部项目只安装一个 Rust AppSDK binary 和文档包即可完成治理流程。
- `.appsdk/` 可提交、可审计、可复现；`.appsdk-control/` 可忽略、可丢弃。
- Rust binary 是唯一正式执行入口。
- 所有生命周期状态、artifact、records、Active、Protected 关系可机器验证。
- 测试、构建、黑盒接入和最终 architecture review 全部有证据。
