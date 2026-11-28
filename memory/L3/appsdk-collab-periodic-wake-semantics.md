<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-21T09:02:50.853313+00:00","entry_kind":"fact","id":"appsdk-collab-periodic-wake-semantics","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":null,"source_refs":["2026-11-21 AppSDK / Collab 主线会话：Jason 对周期唤醒的三条直接要求及对有限周期目标晚读顺延的答复","AppSDK 主线任务 collab-transport-semantics-audit-20261119：periodic-wake-admission-plan.md、remaining-goal-cadence-plan.md"],"tags":["appsdk","collab","ontology","periodic-wake","subscription","user-approved"],"updated_at":"2026-11-21T09:02:50.853314+00:00"} -->

# Collab 周期唤醒：忙时顺延、离线静默、临近耗尽提示续签

## 用户确认的产品语义

周期唤醒的用途是提醒 Agent 检查或继续工作。它由 daemon 调度。订阅与目标归当前逻辑名字，当前宿主承载提供在线、活动和投递事实。

1. 忙时跳过本轮唤醒，顺延一个原周期，到下一轮重新判断。跳过的轮次不累计补发，不扣有限次数，不延长原 TTL。
2. 有限周期接近耗尽时提示是否续签。续签由获授权方显式决定。实施用最后一个可正常发送的提醒附带续签提示，按剩余次数或原 TTL 判断。
3. 已退出、离线的 Agent 不收到周期提醒。恢复使用原有效订阅，按当前状态判断后续轮次。已到原 TTL 的订阅保持过期。
4. 已发提醒被迟读，且下一原定触发点已到或已过时，下一轮从实际读取时间加一个原周期。若原定点仍在未来则沿用原计划。原绝对 TTL 保持，真实读取只计入一次。过期前已接受的提醒仍可读取，读取不会使过期订阅重新生效。

## Object · Behavior · Relation

- Object：周期订阅由项目、逻辑名字和订阅 ID 标识，持有原周期、剩余次数、原绝对 TTL 和调度状态。当前承载提供本轮观察结果。每条已创建提醒有独立消息 ID 和投递/消费回执。
- Behavior：到期检查从 daemon 进入。按此轮当前状态发送、顺延或保持静默。临近有限耗尽时附带续签提示。已接受提醒的消费按原回执幂等推进节奏，绝对 TTL 到期终止后续调度。
- Relation：订阅与目标属逻辑名字。每次尝试指向该名字当时有效的物理绑定。真正的任务截止、到期状态及升级仍由各业务 owner 处理，普通消息和显式单次通知沿其既有合同。

## 执行与证据

这是用户确认的语义来源记录。daemon 订阅生命周期和通知尝试 owner 负责落实。候选 gate 位于周期提醒创建前、投递尝试预留前及消费状态提交。使用公开 `collab goal reconcile`、`collab notify status`、`collab recv`、消息回执与宿主输入核对在线送达、忙时无投递、离线静默、临近耗尽续签提示与幂等消费。持续授权的项目巡检保留独立续行合同。主线任务日志记录各 gate 的实现与实际验收。
<!-- project-memory:end -->
