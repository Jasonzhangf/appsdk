# Rex 与 Collab 的通信兼容

## 本轮目标

独立验证 Rex 的消息输入、接收、状态和跨机器能力，形成可复用基础模块。候选位于 `codex/rex-api-foundations-20261108`。本文件定义后续 daemon 接入边界，实际基础函数和可重复测试见 [rex_api](../../scripts/rex_api/README.md)。

用户指定跨机器能力为重要目标。测试先覆盖本机、用户的 MacBook Air 和 coder1，再依据真实结果决定生产 adapter 的范围。测试 receiver 的 ACK 仅证明该测试进程读到输入。Collab 消费仍由接收方的 `collab recv` 证明。

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

| 当前 owner | 可复用行为 | Rex 接入所需增量 |
|---|---|---|
| `proto.rs` 的 transport 合同 | DesktopBridge、AppServer、Tmux、Dsh 的显式类型 | 增加 Rex kind 与带服务作用域的 endpoint/session/block |
| identity 与 binding owner | 稳定名字、agent sessionID、任务与消息归属 | 将 Rex 地址作为绑定事实，地址变化沿现有 owner 更新 |
| notification sink | 单一 selected transport 分派 | 增加显式 Rex adapter 分支 |
| 通知 attempt 与结果 owner | 接受、失败和结果未知分层 | 映射 Rex 排队结果及部分提交，保留原始错误 |
| 默认 direct-message lease | 订阅归属、target 与 transport 对齐 | Rex 注册和重绑后刷新同一订阅 |
| presence 与 activity owner | 身份存在性和 agent 工作状态分层 | 接入 Rex 进程、程序状态与事件来源 |
| mailbox 与 recv owner | 原消息、接收事务与消费回执 | 继续由正式 recv 提交消费 |

当前实现入口见 [transport 合同](../../collab/src/proto.rs)、[adapter 边界](../../collab/src/adapters/mod.rs)、[notification sink](../../collab/src/server/mod_parts/part_01.rs)、[通知结果](../../collab/src/server/mod_parts/part_05.rs)、[消费事务](../../collab/src/server/mod_parts/part_10.rs) 和 [activity monitor](../../collab/src/server/name_monitor.rs)。

代码审计发现一个需红测确认的批量投递风险。`part_05.rs` 的失败分支仅在 desktop-not-submitted 错误下为整批记账，其余错误只处理 seed message。接入前应验证一次批量发送结果未知时，每个成员均保留正确结果，后续组批不会再次发送已可能到达的成员。该路径处于只读审计阶段，行为风险仍待红测确认。

## 跨机器合同

| 对象或场景 | 合同 |
|---|---|
| endpoint | 每次官方调用显式选择服务，失败保留错误 |
| host_id | 表示服务所见文件域，不能替代物理机器或 Collab 身份 |
| instance_id | 表示本次服务进程代际，变化后刷新目标与观察 |
| session/block | 服务内终端地址，窗口重排使用稳定 ID |
| argv/cwd | 远端启动参数由调用者显式提供，远端回报用于验收 |
| 网络传输 | 使用 Rex 官方支持且已有授权的安全连接，记录实际传输方式 |
| 输入超时或断线 | 保留 unknown 与已完成的部分提交，后续动作等待新证据 |
| 事件重连 | 重读当前状态并保留历史缺口，状态快照不能补造丢失事件 |
| 客户端掉线 | 仅说明连接状态，不能推断 agent 已离线或空闲 |
| 程序 blocked/done | 保留程序自报来源，不能自动完成 Collab 任务或消费消息 |

现有通知与消费图保持独立。跨机器投递仍沿“消息入 mailbox → 选择绑定 → Rex 接受或明确失败/未知 → 目标执行 recv → 消费回执”推进。恢复沿现有 binding 和维护 owner 更新，不新增第二份身份表或消息账本。

## 生产接入的验收缺口

基础测试完成后，还需验证实际 daemon 能调用 Rex，远端真实 agent 能通过正式入口接收和回复，并提交原消息关联的消费回执。还需验证连接中断、服务代际变化、绑定恢复和批量 unknown 的处理。基础模块的测试结果只覆盖其已执行的函数与测试 receiver。

Rex 官方资料见 [Server API](https://www.superlogical.com/rex/docs/automate/server-api)、[Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status) 和 [Remote Access](https://www.superlogical.com/rex/docs/linux/remote-access)。
