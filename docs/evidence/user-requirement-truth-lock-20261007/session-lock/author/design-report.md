报告完成。以下是本轮设计修订的交付内容。

**状态：DONE**

**修改列表（仅允许范围内 5 个文件）**
- `docs/design/user-requirement-truth-lock.md`（主体修订）
- `docs/goals/user-requirement-truth-lock-goal.md`
- `docs/dagpipe/user-requirement-change.graph.json`（0.1.0 → 0.2.0）
- `docs/dagpipe/user-requirement-consumption.graph.json`（0.2.0 → 0.3.0）
- `docs/design/appsdk-authoritative-review-packet.md`（仅尾部追加当前边界说明，未改原文）

**冻结合同落点**
- 授权信任边界：会话原文即授权；执行 agent 提交 `original_text` + `source`，独立 reviewer 对照原会话核验范围；无机器认证/签名/独立账号/权限隔离前置；显式声明不抵抗同 UID 恶意伪造整套文件（第 7、23、48、195、280 行）。
- 唯一 owner `rust/src/main/requirements.rs`，唯一 ledger `.appsdk/requirements.json`，单次原子写保存历史与当前版本，不放 task cache；CLI `appsdk requirements <show|history|apply|verify> [project]`，`apply --input`；输入字段 `request_id/requirement_id/base_version/operation/text/authorization{role,source,original_text}`；只检查必要字段与精确需求/旧版本；幂等复用、变内容拒绝、并发基准冲突单一提交、失败原子。
- 消费：`show`/`verify` 保护状态（旧项目未建立）、`goal.requirements_version` 与候选 `goal_hash` 版本绑定、`review-context` 从唯一 ledger 加载全部需求及历史、合法变更后未更新即 stale、goal 仅作引用；`cancel` 不改 ledger；`reset`/`init fresh` 与普通升级保留 ledger；无锁旧项目兼容（第 5 节，4.2/4.3）。
- T4 改为“缺实际会话授权原文/来源的裸 `confirmed_by` 等拒绝，不宣称机器识别角色 user 伪造”；T9 改为仅覆盖契约范围内 malformed/不一致 ledger 与旧消费版本，不承诺完整同权限篡改；删除额外哈希/物理防篡改诉求；T1–T16 行为验收条目全部保留。

**验证命令与结果**（cwd=`/Volumes/Intel/playground/appsdk/requirements-session-lock`）
- `dagpipe graph validate docs/dagpipe/user-requirement-change.graph.json` → exit 0（`appsdk-user-requirement-change@0.2.0`，5 节点/4 边/5 波）
- `dagpipe graph validate docs/dagpipe/user-requirement-consumption.graph.json` → exit 0（`appsdk-user-requirement-consumption@0.3.0`，6 节点/5 边/6 波）
- `git diff --check` → exit 0
- 附带 `dagpipe graph inspect` 两份 → exit 0，节点/边/波次与文本一致

**过程记录**：`/Volumes/Intel/playground/appsdk/.worker-runs/requirements-session-lock/design/notes.md`（N0 基线读取 → N1 设计文档 → N2 goal → N3 两份 graph → N4 review packet 追加 → N5 验证收尾，每节点含命令、结论、证据）。

**边界与未做事项**
- 未改任何其他文件。`collab/src/server/global_state_tests.rs`、`docs/design/requirement-session-contract.md`、`docs/evidence/.../session-lock*` 为他人并行改动，未触碰、未覆盖。
- 未 commit / merge / push / 安装，未重启 daemon，未初始化源码仓库，未写根目录 `notes.md`，未使用临时脚本。
- 未执行独立 review；本任务的拓扑 PASS 仅证明图结构，产品实现仍待开发与验收。

如有需要，我可以继续把这份报告按 parent 指定格式归档，或对某一段合同表述再收紧。