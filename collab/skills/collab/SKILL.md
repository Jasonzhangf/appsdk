---
name: collab
description: "仅约束已成功注册 Collab 的 peer；master 权限需另行授权。未注册、注册失败或无法注册时独立完成，不适用协作协议。用户明确要求注册时才初始化。"
---

# Collab

## 适用身份与唯一流程

只有当前执行者在本项目成功注册后才适用协作协议。master 还须持有当前项目的明确授权。
加载 Skill、看见 peer 名字、启动 worker 或内部 root 身份均不等于注册或 master 权限。
未注册的一次性 subworker 沿派单工作，不因此成为 peer。首次注册失败保留原错，继续独立工作；
已注册后的通信故障不抹除本人任务、资源或待回复责任。

Collab 只有一个 Skill。`collab/skills/collab/` 是分发源码，`~/.agents/skills/collab/`
是安装产物，宿主链接到安装入口；不为 master、peer、designer、reviewer 建不同 Skill。
运行时使用当前正式安装 Skill 与有效 `role_brief`，旧 worktree 源码不替代运行合同。
源码、安装与 loaded runtime 分别验收，不因文档已更新就宣称线上已生效。

标准流程资源为 [workflow-templates.json](references/workflow-templates.json)。
基础身份只有 master、peer；工作角色是稳定名字的专长，不增加权限。
`role_brief.role_task`、`responsibilities`、`next_action`、`completion_action`
承载展开后的有效流程；`workflow_template` 卡记录来源、target、版本和 revision。
公共规则源码在 JSON 的 `coordination_rules`，运行回执展开到 `role_brief.responsibilities`，
不读取不存在的 `role_brief.coordination_rules`。未配置专长模板时使用身份模板。
复用当前有效回执，只有相关角色、任务或模板变化时更新受影响上下文。

## 注册与恢复

用户明确要求首次注册，或已有身份确需恢复时，在本项目运行：

```sh
collab context
```

cwd 选择项目；稳定名字承载工作，agent sessionID 是当前执行载体。thread、pane、
tmux session、socket 和 server PID 只定位通信/观测。名字续接时 daemon 同步其未关闭任务，
保持任务、证据、资源和未消费消息；已消费历史不重放。缺本地 `.agent-collab/` 不证明未注册。

daemon 核实宿主事实、选择既有 adaptor 并管理身份、绑定和默认接收 lease。
agent 不猜 endpoint、不借别人的 session、不手选历史身份，也不执行额外 route/probe 注册链。
Desktop 普通 CLI 自动接入本会话 bridge，不要求先调用 MCP；DSH 由 gateway 提供当前能力。
仅按回执的 `required_fields`、`reason`、`action` 补真实缺项；成功后直接继续，不固定再查 who/status。
普通恢复不以通信回环为前提；显式通信维修才沿对应诊断路径验证。
具体宿主、名字冲突和授权恢复见 [stable-name-recovery](references/stable-name-recovery.md)。

全局 daemon 由用户管理。agent 不启动、关闭、重启它，不索取维护凭据，不创建第二 daemon。
AppSDK/Collab 不拥有 tmux 生命周期；安装、注册、恢复和巡检不关闭或重启 tmux server，
不终止宿主 pane。失败保留原错，只停真实依赖该能力的工作。

## 普通通信

在成功注册的项目目录发送。已有身份与收件人时直接调用，不先跑 context、订阅发现或恢复。
同项目稳定名字就是地址，无需查 thread/worker ID 或请 master 代发。关联 worktree 可解析到同项目；
目录不确定时回已知注册项目根，不能切到收件人的项目或 home 目录修身份。

```sh
collab sendmessage --to '<peer名字>' --subject '<简短主题>' '<完整正文>'
collab sendmessage --to '<peer名字>' --subject '<问题>' --reply-required '<需要答复的问题>'
collab sendmessage --to '<peer名字>' --subject '<任务>' --reply-on-completion '<完成后交回什么>'
collab sendmessage --to '<原消息from>' --subject '<结果>' --in-reply-to '<原消息ID>' '<实际结果>'
```

正文是位置参数，不是 `--body`。两个回复要求互斥；只有确需答复时使用。
完成后答复不能用 ACK 或中间进度替代；仍有待办的进度消息不提前用 `--in-reply-to` 清除义务。
结果回复不默认附加新回复要求。关联 ID 必须来自真实消息，不猜测。

`sendmessage` 是显式发送；自动通知的批次/节流策略见
[notifications](references/notifications.md)。持久接受、transport 接受、消费、回复分别成立。
不等对方空闲或前一条消费后才发下一条，不轮询发送状态，不自动重放失败或未知副作用。
跨项目 from 是 `<worker>@<project>`，只有当前 master 沿
[master-to-master 通道](references/migration-daemon.md#cross-project-master-communication) 发送；
普通 peer 交自己的 master，不将跨项目地址传给 sendmessage。

## 收件、行动与反馈

| 当前宿主/情况 | 动作 |
|---|---|
| 非 DSH 收到新通知 | `collab recv` 一次，读完整正文并原子消费；不再 ACK |
| 非 DSH 收件响应丢失 | 用原 `receive_id` 执行 `collab recv --receive-id '<id>'` 恢复同一批，不取下一批冒充恢复 |
| DSH 收到 gateway 完整正文 | 直接处理；gateway 独占 Poll/Ack，不调用 recv/ack |
| 只查看某条消息/未读情况 | 按需 `collab msg '<id>'` 或 `collab inbox`；只读不算消费 |

通知只是定位信号，不是新任务或新授权。按原消息区分行动、答复要求和普通信息。
有授权 action 就落实并按约定交回结果；完成后回复须交实际完成结果。
只有信息且未要求回复时不自动回“收到”。系统提醒引用原请求时处理原请求，不回复提醒本身。
读消息、消费、ACK、任务状态更新或对用户汇报都不等于业务答复。
已收到完整正文后不按正文里的通用提示再收一次件。

反馈只保留接收者判断结果、继续工作、恢复失败或验收所需内容：结果、产物位置、关键证据、
剩余问题和下一 owner。必要任务/消息 ID、revision、错误码与恢复标识保留；不复制工具流水、
历史订阅、全部日志或重复流程正文。普通进度只报改变下一动作的变化；无变化不发心跳或催问。
peer 默认向请求方/委托者/master 交回；用户直接询问、要求报告、需要用户决定或重大异常时才面向用户。
处理完协作请求继续自己的未完工作，不因一次回复就结束目标。

## 澄清、决策与阻塞

先依据已有需求、证据和授权自行处理常规技术选择。需澄清时按当前有效
`clarification_route_rule`：事实/接口找负责 peer，设计/需求解释找总设计师，范围、资源和
跨 peer 冲突找 master。仅用户独占关键事实、新授权或用户显式硬约束变化才询问人类。
完整正文由 [workflow-templates.json](references/workflow-templates.json) 的同名规则维护，
不绑定本任务人名，不建立逐级审批/转发链。

求助提供具体问题、已有失败证据、影响及所需决定，不要求先证明根因或找出解法。
临时无回复时继续独立授权工作，只停实际依赖下游。正式阻塞判断需 owner 核实证据、权限和
可行替代；求助不转移交付责任，不为等待制造新任务/轮询。资源冲突见
[resource-waits](references/resource-waits.md)。

## 工作角色与任务分工

注册、设置工作角色、发布任务是独立操作，不是必做三连。已有用户要求直接执行，
可选执行者/reviewer 未指定不构成询问用户的理由。自己的专长设置沿已有授权，master 可按授权
更新项目分工；工作角色、任务 scope 与 master grant 仍由 typed 控制记录分别承载。

```sh
collab context --work-roles '<完整角色列表，逗号分隔>'
collab context --workflow-target 'role:designer' --workflow-template '<四字段JSON>'
collab context --workflow-target 'role:designer' --reset-workflow-template
```

省略 work-roles 保留原值，明确传空字符串清空。项目模板写入/重置仅限当前 master，
target 是 master、peer 或规范化的 `role:<专长>`，不是 agent 名。四字段为 role_task、
responsibilities、next_action、completion_action。多个匹配专长有序生效，不只取第一项。
模板变化不改任务 owner、scope、指定执行者/reviewer 或权限；必要变更显式交接。
master 在已有授权内安排角色变化后的 ready 工作，报告有影响的变化，不重述全套配置。

具体执行/交付/审查请求需要当前任务及验收；讨论、查询和角色设置不要求先建任务。
接单先核对当前 typed scope、指定人和验收；已有有效视图可复用，缺具体字段才查 board。
命名执行者/reviewer 是硬约束；专长偏好只是选择依据。缺条件或超出委派边界先找指派者，
由其在已有用户授权内解决；不能把所有内部任务调整一律变成人类确认。

任务发布、policy、revision、邀请/认领、指定审查和交付命令集中在
[task-worktree-lifecycle](references/task-worktree-lifecycle.md)。邀请不转移责任，接受/认领才转移；
不要用 sendmessage 正文代替 typed 分工，也不通过状态 update 绕过合法接受路径。

## 目标编排与接续

master 和 peer 都是目标编排者，分别管理项目目标与受托子目标。长程子目标交获授权且接受的 peer；
一次性执行/观察沿宿主编排合同使用独立 `codex exec --profile gcm` worker，角色模型和后备同源。
不以其他 peer 全忙作为派发前提，不使用旧 managed-subagent 生产链或会话内 Agent 槽替代。
Plan/Review 与实现按宿主合同分工，Collab 不维护第二套模型、review 或 Git 交付政策。

本人接受的长程目标通过 `appsdk goal subscribe --goal <existing-goal.md> --interval <agreed-period>`
登记，必要时用 `appsdk goal status --json` 查真实结果。任务、board、笔记或 timer 不代替目标注册。
master 不替 peer 持有、更新或取消目标。没有约定周期不臆造，辅助调度不可用不阻断 ready 工作。
新任务按依赖/资源更新本人有限巡检，不重建重复 timers；具体事件、重武装与终止见
[notifications](references/notifications.md#patrol-duties-and-finite-schedules)。

完成后把产物、证据、剩余责任和下一步/idle 请求合成一次交回。当前 master 应给出实际接续或
idle 安排，不能以 ACK 代替；无 master 不虚构审批者，master 不向自己申请。
等待安排期间继续其他授权工作并保留基础消息接收。无人回复不制造额外消息循环。
目标结束或无可执行工作时关闭本人有限调度；基础 direct-message lease 由 daemon 维护。

## 失败与安全边界

- 只保留真实成功、失败、partial 和 unknown；入队、唤醒、消费、Review PASS、合并和关闭不互相替代。
- 一次失败保留原命令、项目、首错及已有持久 ID；无新事实不反复重试或切通道碰运气。
  明确权限拒绝不让 master 代写绕过；只读诊断不授予修复权限。
- 不手改身份、route、token、journal、订阅或任务控制文件。无 v2 生产入口，不依旧 managed 记录接新任务。
- 不接管别人的任务/资源；不共享写树。独占 worktree 和 Close-Before-Open 沿项目/宿主合同，
  dirty、active、shared、unknown 资源保留。只清理本人已满足终点的资源，不执行广泛进程终止。
- 普通恢复、安装或规则升级不触发项目 reset。显式破坏性 reset、名字接管和权限变更沿所属授权合同，
  既有授权不重复索取，沉默不等于批准。

## 按动作读取

| 当前需要 | 唯一说明 |
|---|---|
| 身份恢复、名字继承、宿主事实、冲突裁决 | [stable-name-recovery](references/stable-name-recovery.md) |
| 任务板、policy、交付/审查/集成/清理 | [task-worktree-lifecycle](references/task-worktree-lifecycle.md) |
| 共享资源冲突、具体等待 | [resource-waits](references/resource-waits.md) |
| 订阅/巡检、通知诊断 | [notifications](references/notifications.md) |
| 升级、显式 reset、Desktop/适配器、CEO/跨项目 | [migration-daemon](references/migration-daemon.md) |
| 状态目录与部署归属 | [state-paths](references/state-paths.md) |
| 产品源码、安装与真实行为验收 | [verification](references/verification.md) |

普通发信、收信和回复不加载运维、迁移或全项目验证说明。
