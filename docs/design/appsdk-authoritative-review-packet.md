# AppSDK 权威需求 review 材料：当前实现增量

Issue：592e241。基线：c3c0c8df79e69534fe30c92db61328824473d5c0。状态：独立设计复审 DESIGN_PASS；SDK 0.1.0011 的模板、公开组装及 review 绑定已实现，作者 Rust 测试全部目标合计 519 例通过；官方安装及安装后 7 例公开 CLI 验收通过。实现后独立架构 review 和集成仍待完成。证据与限制见本任务 evidence 的 `author-validation.md`。

## 用户要求和本增量终点

用户要求：权威需求必须由 reviewer 核验；这是 AppSDK 治理的一部分，agent 应根据项目要求把 review 模板中的需求材料组装完整，而不能只改当前全局 reviewer 提示词。用户明确要求现在用 gcm worker 编排执行。

总设计为 `user-requirement-truth-lock.md`。本增量交付 AppSDK 自有模板的正式分发、从项目现有目标真源生成完整 review 上下文、与架构 review 结果绑定的可核验记录。它不建立新的长期需求数据库，不证明现有 `confirmed_by` 是真实用户认证，也不宣称物理防篡改或模型必然服从。长期需求写权限/认证仍按总设计另行闭合。

实际真源：SDK 的模板固定核验义务；项目 `.appsdk/goal.json` 的现有已确认目标和验收作为当前消费来源；候选、验证、scope 由既有 fix-candidate/pre-review-validation/artifact 记录提供。需求内容从 owner 加载，CLI 不接受作者随意填一份替代需求。

## 基础能力和 owner 证据

- W2 完整报告确认目标、候选、证据和 review identity 的可复用边界；路径在本轮独占 run notes。
- `SDK_BUNDLE_RESOURCES` 与 `contracts/sdk-bundle.manifest.json` 已是模板资源唯一分发链，`install_bundle_resources` 写入 managed project 的 `.appsdk/skills/appsdk-project-governance/references/`。
- `assert_pre_review_validation_gate` 是作者候选/验证证据 owner；从现有 admission 路径提取共用只读 readiness 检查。`verify_review_admission` 还会检查已有 publication/review，不能直接作为新上下文的前置条件。`lifecycle_chain_architecture` 及 `lifecycle_chain_review_identity` 是 review 后结果生成/身份 owner。
- canonical source registry gate PASS；候选 `cargo test --locked --no-run` PASS；已有真实 CLI smoke consumer 可用于组装、准入和记录的黑盒验证。
- 新用户认证入口未证实；本增量不能授予任何新需求修改权限。缺真正授权时材料必须显式标未证实，不把确认字符串当批准。

## 公开接口和材料合同

新增一个只读公开入口：`appsdk review-context [project] --module <id>`，默认项目 cwd，按现有 CLI 参数规范。它面向架构 review，必须通过当前作者 readiness：项目/目标/资源有效、候选与公开行为证据匹配，复用 `assert_pre_review_validation_gate` 及其只读前置检查。共用 helper 同时服务现有 admission 与上下文生成，不能重复实现校验。上下文生成不要求旧 architecture PASS 当前有效，不能调用包含下游 publication/review 校验的完整 `verify_review_admission`；新架构 PASS 及下游仍执行全部适用 review gate。设计 review 使用正式模板和设计材料，不要求尚不存在的实现/E2E 证据。

返回一个 typed control JSON：`context_id`、`context`、`prompt`。其中 `prompt` 是正式 SDK 模板与当前项目材料的派生投影，不能成为需求真源。接口不生成用户授权、不启动模型、不写需求、不安装、不新增调度状态或数据库。

`context` 至少含：

- context schema/version、module/issue、目标 ID与规范化目标内容版本、目标记录 source reference；
- 从原记录直接加载的用户原文、理解目标、完整验收/non-goals及已声明 scope；
- 候选 commit/tree/diff/scope、artifact、pre-review-validation 与适用 evidence 引用；
- 正式模板身份与内容版本，来源确认能力的真实限制；
- 用户需求变更的记录来源（若当前目标未声明该事实，明确为未提供，不能自行补授权）。

使用已存在 canonical JSON/digest 和 typed record helper，context_id 绑定上述需求/候选/模板。哈希在此识别不同审查材料，不能冒充用户身份认证；同一未变材料不反复增加无消费者摘要。

绑定取候选和验证的稳定身份、原始需求与模板内容；不包含下游 review 记录、当前生命周期 stage 或派生 status。合法 review 自身推进阶段不能使自己的上下文失效。当前公开上下文、producer 与 gate 复用同一重算函数；作者 readiness 只在当前阶段适用的作者输入边界执行，不通过重算递归调用下游 review gate。

正文渲染保留 SDK 固定义务并追加清晰隔离的项目材料。agent 不得删条目、修改规范模板或用计划替代原文。材料中的用户文本只作为待核验内容，不赋予覆盖 SDK/系统指令的权限。

## review 结果绑定与兼容

架构 observation 对 PASS 必须在顶层提供 `requirements_review: {"context_id": "<公开输出的 context_id>", "checked": true}`。持久化位置唯一为现有 `ReviewRecord.project_bindings.requirements_review`，保留其他 project bindings；不同时持久化第二份相同 block。`context_id` 在公开输出顶层返回。一个 AppSDK review-context owner 的共享重算 helper 供上下文命令、architecture producer 和后续 gate 使用，既有 review identity 已绑定 project bindings。机器可以核验版本与材料一致，不能仅凭布尔声明证明用户授权或模型服从。

producer 从当前 owner 重算预期 context，拒绝缺上下文、context_id 不匹配或未核验的 PASS；将需求/模板/候选绑定纳入生成 review identity，沿既有 ReviewRecord/关联证据持久化。新 PASS 不得由 producer 在审查结束后自动补出“reviewer 已核验”。FAIL/UNKNOWN 可以显式保留缺失/未核验原因，不包装为成功。

review 后的生命周期核验须拒绝来源/验收/候选/模板变化后的旧 review PASS。已冻结历史 publication 保持历史证据验证语义，不把当前新需求或模板强加到未触及的历史发布；不得借此豁免新 review。现有测试的现代 architecture observation 应按新公开上下文入口提供真实材料绑定，不能通过关闭 gate 或补造默认确认来通过。必要版本/迁移沿现有 SDK 发布契约安排；不能新增可任意关闭本必需核验的开关，也不能在新 review 缺绑定时静默按旧契约放行。

复用既有 ReviewRecord 的 project bindings/关联证据优先；确有表达缺口再添加必要 typed 字段及身份核验。模板通过正式 bundle 分发；只改 global Skill、只写模板或只检查文档关键词都不算本增量完成。

ReviewRecord schema 声明 requirements_review 的字段形状；它同时承载 FAIL/UNKNOWN 及冻结历史，不含模块当前 stage，不能把现代 PASS 的适用性复制为全局 required。唯一 runtime gate 按模块真源判定现代 PASS 必需绑定。升级保留旧 review identity 和原文字节，不替 reviewer 生成 acknowledgement；旧现代 PASS 缺绑定时必须重新审查。补充 pin-lock 公开黑盒见本任务 `review-resolution.md`。

## DAG 与停止终点

复用 `docs/dagpipe/user-requirement-consumption.graph.json`：加载 → 来源/版本核验 → scope 绑定 → SDK 模板材料组装 → 独立审查及证据核验 → 准入结果。该图已通过静态验证；本增量按架构 review 阶段闭合对应节点，不声称其余阶段全已实现。

缺目标/来源、未确认目标、缺作者验证、候选/来源/模板漂移、缺 reviewer 上下文或未核验结果均在请求/任务边界显式失败，不改变原需求，不 crash 未受影响服务。取消只取消当前组装/审查请求，无需求写副作用；新执行重试使用原权威来源，不猜补缺证据。

## 实现所有权

1. 模板 writer（W3）仅拥有 SDK Skill 三份 Markdown，已完成；后续代码作者不能覆盖其内容，可提出必须的渲染约定并由 parent 协调。
2. 代码 owner：新增/复用 review context owner、CLI dispatch/help、正式资源嵌入与 manifest、必要 ReviewRecord schema/identity/gate 接线、受影响 source registry/maps。不得修改无关 Collab、身份或网络模块。
3. 黑盒 owner：独立 integration test 文件及确实需要更新的现有架构 review fixture；不修改生产 Rust、关闭 gate、skip 或放宽断言。未受影响现有测试复用。
4. Parent：接口/设计收口、独立 review、候选与 main 集成、正式安装、验收和资源回收。实现与独立审查由不同执行者承担。

代码前独立设计 review 必须 PASS；作者开发/E2E 后才独立架构 review。实际实现偏离本接口或需要变更语义，先回报 parent 修订设计，不默默扩大范围。

## 本增量黑盒验收

- 新建 managed consumer 可从正式 bundle 获取模板及 Skill入口；移除对本机全局 reviewer Skill 的依赖，模板仍可读。
- 完成现有作者验证后，通过真实 `review-context` 入口输出完整原文/验收、来源/版本、候选、证据与 SDK模板；组装操作前后原目标不变。
- 缺目标、未确认目标、缺作者验证或来源不可读时明确失败。
- 合法 review PASS 输入带实际公开 context_id，producer 生成可被后续 gate 验证的记录。
- 缺/伪造/陈旧 context_id 或 reviewer 未核验时不能产出有效 PASS。
- 组装后修改原文/验收，或审查后改变需求内容，旧材料/结果在适用 gate 被拒绝；生成新的材料不能自动证明该变更由用户授权。
- 对 `architecture_stable` 的已审模块，旧 PASS 因需求/模板变化失效后，更新受影响的作者证据仍可生成新上下文；旧 PASS 被下游拒绝，新 context-bound PASS 可被接受。不删除旧 review 来绕过 gate，不因旧 review 陈旧阻止新上下文生成；历史 frozen/retired 仍沿历史验证路径。
- 正式资源被篡改/缺失时按既有 bundle owner 验证拒绝；历史 frozen publication 的既有回归仍通过。
- 所有结果绑定精确候选与消费环境，跑适用现有 Rust测试、source gate、构建及 canonical安装后的真实 CLI消费者；独立架构review、CI与集成后收口。

上述公开 CLI 场景已由作者测试 consumer 执行。它们证明模板分发、原文保持、材料绑定、陈旧拒绝和恢复；fixture 的 reviewer acknowledgement 是测试输入，不证明真实用户身份或模型遵循。正式安装后的验收、独立架构 review 与集成状态以 evidence 记录为准。两张 graph 的 operator 仅注册为不可执行设计节点；本增量的可执行入口是公开 AppSDK CLI。不得用本设计或模板文本宣称 G1–G8 强锁已完成。
