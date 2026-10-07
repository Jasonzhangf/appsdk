# G9/T16 作者验收与审查材料

Issue：592e241。Base：c3c0c8df79e69534fe30c92db61328824473d5c0。候选分支：codex/authoritative-review-template-20261007。SDK 0.1.0011；Rust package 0.1.11。2026-10-07 作者验证完成，进入独立架构 review；尚未宣称 review、CI、集成或资源收口完成。

## 权威输入与适用范围

逐字用户输入见 `user-input.md`。完整目标见 `../../design/user-requirement-truth-lock.md`；本次增量设计见 `../../design/appsdk-authoritative-review-packet.md`。无用户需求修改或验收降低授权。G1–G8 的长期版本、可信身份/变更授权、独占写权限与 reset 保留仍是 gap。不能用本增量的材料哈希和显式 acknowledgement 代替它们。

本次 scope 是 SDK 自有模板、公开材料组装、显式 reviewer acknowledgement 的持久身份绑定、下游陈旧拒绝与恢复。独立设计 W4R PASS 在 `execution/design-review-revised.md`；首次 FAIL 与修订也已保留。SDK 源码根不是 managed consumer，未对它 init 或重置治理，也未注册 Collab、重启 daemon 或创建 goal 订阅。

| 用户要求 | 本次实现和证据 | 当前边界 |
| --- | --- | --- |
| AppSDK 治理拥有 review 模板，不能只改当前提示词 | SDK source template、bundle manifest、embedded resource，真实 consumer 分发测试 | 固定义务进入SDK产品；backend自动 dispatch adapter 不在本增量 |
| agent 从项目真相组装完整 review 材料 | `review_context.rs` 从原 goal、候选、validation、scope、evidence 读取；公开 JSON 含原文、验收、版本、SDK模板与限制 | 当前goal合同不含可信用户变更授权/旧需求版本，明确 not available |
| reviewer 必须核验权威要求 | template 的逐条核验义务，PASS 必须显式返回当前context_id与checked；binding先纳入review identity | CLI不能证明模型内心行为；adapter不能自造确认，fixture确认只是输入 |
| 未修改的需求持续有效 | 原文保持、陈旧context/PASS拒绝、重新审查恢复测试 | 不证明物理防改或仅用户有写权限；完整锁仍待后续实现 |
| 用GCM worker编排执行 | `execution/`内审计、模板、设计review、红测交付；parent节点笔记记录后续实现worker取消和owner转交 | 无产物的取消worker不计成功；生产实现和架构review分离 |

## 作者开发与公开入口验证

Rust 全部 targets 合计 519 例 PASS。完整命令 `cargo test --locked --manifest-path rust/Cargo.toml` 的第四次运行通过八个目标共517例后，最后sdk_0009目标因当前版本旧断言失败，整体 exit 101。修正该fixture后单独运行 `--test sdk_0009_migration` 的两例全部 PASS，exit 0；此前通过的生产/其他目标证据继续有效，未改生产代码。原始完整日志压缩为 `execution/rust-full-attempt4.log.gz`，补验为 `execution/sdk-migration-green.log.gz`。不把完整命令写为exit0。

`cargo test --locked --all-targets --manifest-path dagpipe/Cargo.toml` PASS；运行时源码未变。两张图由现有registry嵌入且仅绑定设计operator，执行明确报 `DAGPIPE_DESIGN_OPERATOR_NOT_EXECUTABLE`。图校验证据只证明拓扑。

正式build与官方安装：`cargo build --release --manifest-path rust/Cargo.toml` PASS；`scripts/install-global-appsdk.sh` PASS。原始 `release-build.log`、`install-global.log` 已归档。固定安装入口 `/Users/fanzhang/.cargo/bin/appsdk` 为0.1.0011，已与候选release binary核对SHA-256相同：`2d5b64065f1b985cc034043d2016908bf4be2cc9c052c7c5ba70a209de2bb59a`。模板source与全局Skill安装文件 `cmp` exit0。此处哈希用于产物来源核对，不是认证门禁。

从同一公开CLI consumer入口执行安装后E2E：每个测试创建独立 managed project，从安装binary输入真实命令，断言stdout、退出状态、原goal及公开record/gate结果。没有mock生产内部调用。固定 `APPSDK_TEST_BINARY=/Users/fanzhang/.cargo/bin/appsdk`；共享构建目录只承载test consumer。

```sh
cargo test --locked --manifest-path rust/Cargo.toml --test cli_smoke review_context -- --nocapture
cargo test --locked --manifest-path rust/Cargo.toml --test cli_smoke review_gate_rejects_tampered_goal_and_requirements_binding -- --nocapture
cargo test --locked --manifest-path rust/Cargo.toml --test cli_smoke architecture_pass_rejects_stale_context_after_goal_material_change -- --nocapture
```

以上分别5、1、1例PASS，exit0。安装后日志 `execution/installed-*.log.gz` 保留真实输出：

- 正式模板分发、goal原文字节保持、完整材料输出。
- 缺goal、无确认、缺作者证据、缺/改模板显式拒绝。
- 合法明确ack生成可校验review identity；缺、假、未check的ack不生成PASS。
- 组装后goal变化拒绝旧ack；review后goal或binding篡改拒绝旧PASS。
- architecture_stable旧PASS失效后仍能生成新context，通过公开architecture→effectiveness→merge→promotion producer恢复，完整admission PASS；旧attempt history保留。

测试中的合成reviewer输入不是真实模型输出，也不是用户认证。独立架构review另行执行。Rust format、JSON、source registry及diff检查均PASS，source registry未放宽1500行门禁。唯一未触及warning为communication/store_runtime未使用import。

## 候选身份和证据复用

验证在当前独占候选进行。正式候选提交将把所有实现、测试、模板、maps与本目录证据一起固定；该提交SHA记录在独占run notes及独立review工具scope中。作者完成后无生产、模板、maps、测试或build输入变化才可复用这些结果；后续只有状态/receipt文档变化时核对源树等价即可。不得将base SHA冒充实现SHA。

本次可见中断：官方installer成功后Intel盘短暂掉线，于14:06:52Z恢复，候选、run及安装来源已恢复核对。见 `implementation-checkpoint.md` 的历史记录。无证据丢失或强制prune；安装后7例是在恢复后的当前入口运行。

## 独立审查和交付后续

正式Codex/AGY审查应直接读取用户原文、两份设计、图产物、源码与上述作者证据。请将需求来源及逐条核验结论写入既有JSON evidence字段；当前源码根没有managed context，不能对它制造goal或requirements_review。review结果由正式controller判定，不由作者填写。

通过后再完成适用CI、最新origin/main组合、owner正确集成、push回执和自有资源回收。主树他人的docs/collab.md不得覆盖。完整需求锁仍为未完成目标。
