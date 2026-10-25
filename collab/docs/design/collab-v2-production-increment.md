# Collab V2 首个生产增量

本设计将 V2 原型中已验证的能力接入当前生产 owner。原型的内存 queue、manager map
和 shell 进程样本不能证明生产调度已实现。实现与验收逐项推进，不复制另一套控制面。

## 目标与执行顺序

| 项目 | 最小交付 | 当前边界 |
|---|---|---|
| 首次注册授权 | 用户预授权项目策略后，第一个符合条件的注册原子取得现有 master grant；策略只消费一次 | 使用当前身份授权入口及 journal；默认无策略的项目保持原行为 |
| 稳定 manager 接管 | 同一授权槽位的新载体接续名字和未结责任，旧载体失去相应写入权 | 生产仓库尚未提供可信的宿主槽位激活发布者；先补该接缝，不建无消费者的 manager 注册表 |
| Desktop 通信 | 按名字发送、native 接受、目标消费、关联回复、发件人消费 | 每一步用实际所属回执证明；bridge 接受不能代替消费 |
| DSH 通信 | 相同消息语义，gateway 独占 Poll/Ack | 请求提交后读错误属于结果未知；真实 gateway/runtime 可用性与源码测试分开验收 |

首次注册控制与 transport 修复可并行。控制状态、名字和授权共享文件由一个 writer
修改。每个独立分支通过 Git 合入唯一集成分支。完整可交付增量自检后进行一次独立
整体审查，再提交和推送。受影响图先于 gate、红测和实现补正。

## 控制面唯一归属

注册及其副作用复用 `typed_dispatch_register` / `commit_command_locked`；角色事实
来自 `MasterGrant`，名字与工作继承来自 name owner。operation journal 记录操作
进度和所属 command 回执，不能另存一份身份、策略或授权真相。

首次注册策略使用现有 `context --approve-grant` 授权入口，不增加平行注册命令。
授权沿现有明确用户 action、非空 authorization 和 project/app scope 边界处理。
策略 authoring 必须有公开入口及真实注册消费者，不能只增加结构体和单测。

注册在同一状态锁内判定：策略有效、未消费、没有先前注册历史及 incumbent 时，
注册、现有 typed grant 和策略消费进入一个 command。并发以服务端提交顺序决定。
offline/unknown 不等于无 incumbent；clear 不重新开放已消费策略。明确选择普通
peer 时保留该选择，并结束本次首次注册机会。返回丢失应查询原 command，不能再授予。

manager 槽位必须由所属宿主提供稳定槽位、项目和有序 activation。cwd 只定位项目；
窗口标题、pane 或任意 agent 自报字段不能赋权。后续实现复用名字、binding generation
和现有 grant owner，原子转移未关闭工作、未消费消息及相应唤醒目标，并 fence 旧写入者。
已消费消息和已关闭工作保留历史。fence 不表示旧进程已停止。缺少宿主发布者时，
该项保持未交付，其余项继续。

## 通信的最小反馈

一次发送只回原消息 ID、是否持久接受以及当前需要调用者处理的投递结果。正文保留。
注册不回放已消费通知，也不默认展开所有订阅、历史诊断或角色规则。收件只返回本次
消费的消息和必要回执；回复关联原消息，不自动引发新的 ACK。诊断按请求读取。

DSH 投递复用 `collab-dsh-delivery.graph.json` 的单源单汇对象流：

```text
持久消息 → 当前收件绑定 → 范围与端点核对 → gateway 提交 → 独立投递结果
```

提交前明确连接失败可以证明未投递。显式拒绝保留拒绝原因。提交后的写入、读取、
超时、EOF 或畸形响应不能证明未投递；结果未知时保留 durable mailbox，不自动重发
或切换 transport。原始错误由 adapter 保留，唯一分类供下游复用。

Desktop 收件沿公开 receive 入口。DSH 收件由 gateway Poll/Ack 完成；DSH 错误回执
不得建议调用已禁止的 CLI recv/ack。双方均用真实消费与关联回复完成往返验收。

## 最小充分验收

- 控制：公开策略入口到真实注册；首个与后续注册、并发唯一 winner、无策略、拒绝、
  错误范围、已有/unknown incumbent、clear 后不复活、明确普通 peer、原请求重试、
  完整 replay 和持久失败。只保留能区分错误实现的反例。
- DSH：网关收到完整请求后令读取失败，先证明错误分类及错误重发，再验证修复；
  同时保留连接前失败和显式拒绝控制。公开 Send 的原消息、正文及消费状态必须保持，
  replay 和重复 Send 不能再次投递未知结果。
- 真实宿主：发送者、接收者、native/gateway 回执、目标消费、关联回复与发件人消费。
  fixture 只证明协议和实现，不能替代该项。运行实例缺失时记录明确失败，不改写为通过。

现有 journal 的 incomplete command 是明确故障，不宣称自动恢复；不截断用户 journal
或补造 completion。全局 daemon 由用户管理。测试使用隔离 fixture，源码合入不代表
已安装、用户已激活或所有 live 路径已通过。

## 后续增量

manager 宿主发布接缝闭合后，再接具体 host 的 execution attempt。task 的 working
只表示业务状态。启动前保存 intent，running 来自真实 host 回执；启动未知先 inspect
同一 attempt。取消请求、lease 到期都不表示进程停止。暂不引入自动重派、完整布局、
跨机迁移、Vikunja 或通用 provider 框架。
