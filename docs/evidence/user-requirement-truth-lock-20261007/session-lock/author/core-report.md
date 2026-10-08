自清理完成（5 个临时验证目录已移除，项目内无残留）。交回 parent。

**交付**
- `rust/src/main/requirements.rs`（新增）：会话授权需求锁唯一 owner，实现全部冻结 API：`requirements_cli`、`read_requirements_if_present`、`assert_requirements_current`、`requirement_review_material`。
- `contracts/records/user-requirement-request.schema.json`（新增）：apply 请求合同（字段、role=user、text 条件必填）。
- 未改动其他文件；未 commit/push/install/restart；未写全局 memory；未撤销他人改动。

**验证**
- `cargo check` / `cargo build --manifest-path rust/Cargo.toml` 通过；唯一 warning 来自他人 `communication/store_runtime.rs`。
- `rustfmt --edition 2021 --check` 干净；schema 是合法 JSON。
- main.rs 已由其他 owner 接上 `mod requirements;` 与 `Some("requirements")` 分发，签名与冻结合同一致，CLI 已真实可跑。
- 真实 CLI 黑盒（临时项目）：show 空 `not_established`；create→active v1；原样重放 `reused:true`；同 id 改字 `REQUIREMENTS_REQUEST_CONFLICT`；replace/revoke/恢复版本链正确；多需求 show 每条仅最新版；cancel 不写 ledger 且带 text 拒绝；裸 `confirmed_by`、缺 `original_text`、错 project、create 既有 id 均显式 exit1。
- 自查修复三处实现缺陷：reuse 误判冲突、show 误含被覆盖旧版、replace 不能恢复 revoked。

**交回**
- candidate SHA 由 parent 提交；`cargo test --test cli_smoke requirement_lock` 由 testworker/parent 统一重跑。
- 过程笔记与报告：`.worker-runs/requirements-session-lock/core-impl/notes.md`、`report.md`。
- 阻塞项：无。