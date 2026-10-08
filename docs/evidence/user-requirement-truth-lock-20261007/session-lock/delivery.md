# 会话授权需求锁交付回执

用户明确边界：会话中的真实用户指令足够授权。执行 agent 记录原文与 source，reviewer 核验指定范围；机器仅检查必要结构、版本、提交一致性和消费绑定。

当前版本：AppSDK 0.1.0012；开发 base `8555a74e5e46de10136fda9539ae044d3e8bdfbc`。作者验证、安装和独立架构review已完成；CI、main集成及最终worktree回收按后续收口记录确认。

作者准入完成：完整Rust套件exit0（含351个CLI用例），DAGpipe all-targets exit0/28，安装后受影响pin/升级37组exit0；源码/installed需求11组证据保持有效。source-registry、Rust/DAGpipe格式、全部contract JSON及SDK版本一致性通过。独立架构review与最终集成回执待下方收口记录。

独立架构review：AGY task `requirement-session-architecture-20261008`，controller `completed/pass`，exit0、findings为空；已验staged源树 `0251a454b672d850540323135ffb55603b4c2364`。完整提示词含真实用户原文和最新会话授权指令；输出与controller证据保存于architecture-review。review之后只新增交付文档/证据，产品实现未改变。

临时测试资源：从本任务Initialized repository日志和对应registry项目来源确认ownership后，移除689项自有临时资源，无失败；其他owner资源未删除。Collab worker额外创建的5个/tmp文件按原创建事件确认后已归档所需日志并移除。最终候选worktree和本轮worker run仍用于CI/集成，完成后回收。

官方安装已完成：`$HOME/.cargo/bin/appsdk` 返回 `appsdk 0.1.0012 (rust)`；最终canonical与候选release二进制SHA256均为 `d8470cd7ddf6ca0833b6d7a780781c9a1c0f5ec2c9809702929c9f31a7023fbb`。安装脚本也发布同源AppSDK Skills。此前installed需求11组全部通过；随后仅修正旧maps迁移恢复分支，当前canonical的受影响pin升级黑盒结果在author记录。没有daemon变更，无需daemon重启。

## 已完成主线与原始 gap

| 原始 gap | 当前实现与公开行为证据 |
| --- | --- |
| G1 / G5 长期需求及历史 | requirements.rs 唯一owner；`.appsdk/requirements.json` 保留请求/结果链；create/replace/revoke/restore、多条目和原文测试通过 |
| G2 会话授权 | 原文/source/role=user 请求；缺授权、裸confirmed_by、错角色、错项目/条目/基准拒绝；自然语言范围由agent/reviewer核对 |
| G3 / G4 消费绑定 | goal.requirements_version；compile/verify/review/Guidance 在既有goal读取边界调用同一owner；旧版本明确失败 |
| G6 升级与reset保留 | 普通init、reset/fresh-init原字节保留；从官方0011产生的真实fixture pin升级0012后账本相同 |
| G7 行为覆盖 | `rust/tests/cli_smoke/part_24.rs` 11组真实CLI黑盒已通过；包括并发、写失败、取消、数据不一致和升级 |
| G8 源码仓库默认未启用 | 保持源repo与managed consumer边界；未初始化源repo，无锁旧项目返回not_established |
| G9 正式review治理 | SDK自有模板分发、review-context载入全部需求/历史及来源、既有ReviewRecord绑定；旧context拒绝，新context可生成记录 |

T1/T2/T5/T12：原文、多条目、修改、撤销和恢复；T3/T4/T6/T7：缺授权、非user、错项目/条目/旧版本，历史不改变；T8：init/reset/fresh-init/官方0011升级保留；T9/T15：不一致result、错误项目账本、读取错误、旧goal/旧review；T10/T11：幂等重放、同ID不同输入、并发只一次提交、实际写失败不产生半历史；T13：执行agent只提交明确用户指令，reviewer核验自然语言范围；T14：无锁兼容；T16：SDK正式模板、完整review材料及记录绑定。

第一轮7组5通过2失败，原因是测试误用可复用模板编译入口和无dirty的fixture commit；修正后7/7通过。补充红测证明owner一致性问题后修复，再通过9组、最终11组。失败日志保留，不作为PASS证据。

## 使用入口

从用户真实会话构造请求，调用 `appsdk requirements apply <project> --input <json>`。读取用 `appsdk requirements show <project>`，完整历史用 `history`，保护状态用 `verify`。replace/revoke的base_version取指定条目的当前版本；总version用于goal.requirements_version。合法变更后更新任务绑定并重验受影响行为，旧review不能继续复用。

完整请求示例与冻结合同见 `../../../design/requirement-session-contract.md`。两份项目graph和设计文档保留唯一需求写入与只读消费两条SESE路径。
