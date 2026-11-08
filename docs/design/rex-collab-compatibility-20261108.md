# Rex 与 Collab 的通信兼容

## 本轮目标

独立验证 Rex 的消息输入、接收、状态和跨机器能力，形成可复用基础模块和 Collab adaptor。候选位于 `codex/rex-api-foundations-20261108`。本文件记录候选 daemon 的接入边界，基础函数和可重复测试见 [rex_api](../../scripts/rex_api/README.md)，组合入口见 [Collab 验证映射](../../collab/docs/verification-map.md)。

用户指定跨机器能力为重要目标。候选已覆盖本机、用户的 MacBook Air 和 Linux coder1。每台机器保留自己的 daemon、身份和 mailbox。测试 receiver 的 ACK 证明该测试进程读到输入，Collab 消费由接收方的正式 `collab recv` 提交。

## 基础函数与对应行为

| 基础能力 | Rex 入口 | 必须验证的行为 |
|---|---|---|
| 能力发现 | `server.version`、`api list` | 使用目标服务的实际 schema，缺能力只影响对应动作 |
| 服务事实 | `server.status` | 保留 endpoint、host_id 和 instance_id，区分文件域与服务进程代际 |
| 主机发现 | `hosts.list` | 空目录合法，目录条目与连接目标分开 |
| 精确寻址 | `session.list`、`session.list_blocks` | 完整 session/block ID 属于显式选定服务 |
| 文本提交 | `terminal.write` | 保留 Unicode、换行和空格，返回排队字节数 |
| Enter | 官方 `send-key enter` | 与文本回执分开，文本结果未知时停止后续按键 |
| 输出读取 | 官方 `capture` | 复用官方分页，读取当前保留的完整 screen/scrollback |
| 进程与目录 | `terminal.process`、`terminal.pwd` | 返回真实来源，远端路径使用远端语义 |
| 程序状态 | `terminal.program_status` | 空记录保留为空，程序自报与 agent 权威状态分开 |
| 事件观察 | 官方 `events --json` | 有限观察、精确目标过滤、原始 envelope 和断线缺口 |
| 独立接收 | 测试 receiver 的逐 case ACK | 比较实际读取的完整字节，排除输入回显 |
| 生命周期 | 自有 session 创建与销毁 | 仅使用本次创建回执，所有失败均收口自有资源 |

## daemon 可以复用与需要增加的部分

| 当前 owner | 可复用行为 | 本轮候选接入结果 |
|---|---|---|
| `proto.rs` 的 transport 合同 | DesktopBridge、AppServer、Tmux、Dsh 的显式类型 | 已接入 Rex kind 与带服务作用域的 endpoint/session/block |
| identity 与 binding owner | 稳定名字、agent sessionID、任务与消息归属 | Rex 地址作为绑定事实，地址变化沿现有 owner 更新 |
| notification sink | 单一 selected transport 分派 | 已接入显式 Rex adapter 分支 |
| 通知 attempt 与结果 owner | 接受、失败和结果未知分层 | 映射 Rex 排队结果及部分提交，保留错误与未确认的清理结果 |
| 默认 direct-message lease | 订阅归属、target 与 transport 对齐 | Rex 注册和重绑刷新同一订阅 |
| presence 与 activity owner | 身份存在性和 agent 工作状态分层 | Rex 程序状态与事件投影 activity，presence 保留其独立来源 |
| mailbox 与 recv owner | 原消息、接收事务与消费回执 | 继续由正式 recv 提交消费 |

当前实现入口见 [transport 合同](../../collab/src/proto.rs)、[Rex adaptor](../../collab/src/adapters/rex.rs)、[notification sink](../../collab/src/server/mod_parts/part_01.rs)、[通知结果](../../collab/src/server/mod_parts/part_05.rs)、[消费事务](../../collab/src/server/mod_parts/part_10.rs) 和 [Rex monitor](../../collab/src/server/rex_monitor.rs)。

批量 unknown 的风险已通过真实红测确认并修正。当前候选为本次提交的每个成员记录真实结果，后续组批保留已可能到达的事实。有限 Rex 操作及其依赖阶段、child 和 reader 收口共用一个绝对期限。到期后保留已确认阶段，并明确报告尚未确认的清理结果。

## 跨机器合同

| 对象或场景 | 合同 |
|---|---|
| endpoint | 每次官方调用显式选择服务，失败保留错误 |
| `project.name` | 由本机 daemon 解析本机项目与稳定名字 |
| `machine.project.name` | 由源 daemon 选择已声明机器，目标 daemon 解析自己的项目与名字 |
| Collab 机器链路 | 定向认证的私有 SSH Unix socket 连接，各机独占身份、mailbox 和消费状态 |
| host_id | 表示服务所见文件域，不能替代物理机器或 Collab 身份 |
| instance_id | 表示本次服务进程代际，变化后刷新目标与观察 |
| session/block | 服务内终端地址，窗口重排使用稳定 ID |
| argv/cwd | 远端启动参数由调用者显式提供，远端回报用于验收 |
| 网络传输 | 使用 Rex 官方支持且已有授权的安全连接，记录实际传输方式 |
| 输入超时或断线 | 保留 unknown 与已完成的部分提交，后续动作等待新证据 |
| 事件重连 | 重读当前状态并保留历史缺口，状态快照不能补造丢失事件 |
| 客户端掉线 | 仅说明连接状态，不能推断 agent 已离线或空闲 |
| 程序 blocked/done | 保留程序自报来源，不能自动完成 Collab 任务或消费消息 |

跨机器投递沿“源 daemon 登记操作 → 目标 daemon 认证与解析 → 目标 mailbox → 所选 Rex 绑定通知 → 目标正式 recv → 消费回执”推进。关联回复返回原来源。相同 operation 重试复用原引用，Lookup 与获准的 Put 共用期限。权威接受结果与附属 transport_error 分开返回。恢复沿原 binding 和维护 owner 更新，身份与 mailbox 各有唯一 owner。

## 候选验收与生产边界

候选已通过真实 Rex API、本机正式收发、两个独立 daemon 的真实 SSH 通信、Air 与 coder1 的物理双向正式消费和关联回复，以及本机真实 Rex 前端。响应丢失后的 unknown 恢复、去重、服务代际变化、重启恢复、批量 unknown、背压取消和累计期限均有对应行为验证。consumer 使用测试 receiver 和正式 Collab CLI。

生产接入还需 runtime 安装批准与用户激活、现有 tm-iterm2 布局和会话迁移验收，以及真实 QQ 入站到远端 agent 的原路回复验收。当前 QQ 联邦组合使用隔离 sink，独立 helper 已取得真实平台出站接受回执。

Rex 官方资料见 [Server API](https://www.superlogical.com/rex/docs/automate/server-api)、[Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status) 和 [Remote Access](https://www.superlogical.com/rex/docs/linux/remote-access)。
