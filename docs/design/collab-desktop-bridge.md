# Collab Desktop bridge

状态：产品接入设计与候选实现。基础通信和一次真实 idle 唤醒已有证据；正式安装、
MCP 配置加载及 Collab daemon 组合验收必须另行报告。

Desktop 的当前聊天不一定由独立 managed AppServer 承载。对那个 AppServer 的
`thread not found` 或 `notLoaded`，不能推导 Desktop 官方工具不能访问聊天。
本增量使用正常 Desktop-owned MCP bridge。daemon 只连接 bridge 的本地 UDS；
bridge 连接宿主提供的 app-tools pipe，并调用实际公布的原生工具。

实验已验证普通 tmux 脚本经正常受管前台 bridge，向当前聊天投递并收到实际 ACK；
随后在聊天 idle、原 turn completed 后发送第二条消息，实际启动新 turn 并继续工作。
这些事实见任务记录 `bridge-proof-20261023`。没有使用 detached keeper、忽略终止
或伪造 `cli-turn`。正式服务沿相同正常生命周期实现。

## 身份与上下文

名字承载工作，agent sessionID 是执行载体。bridge UDS、tools pipe 和 thread
定位通信，不是身份。Desktop transport 与 AppServer WebSocket transport 分开。
原有 tmux、TUI 和 DSH 不因此改变协议或恢复 gate。

真实 caller thread/turn 来自本次 MCP `tools/call` 的 executor metadata。
官方 MCP 接口从 `request.params._meta` 读取 `x-codex-turn-metadata` 或对应
`openai/threadId`、`openai/turnId` 字段。不能以普通工具 arguments 或历史日志
替代这个来源。真实原始发起 turn 可以随已授权请求保留，不能伪装成新的活动 turn。
通知 target 则由 Collab 当前注册收件 route 决定，不能由外部发送者指定上游身份。

bridge 只开放已绑定收件范围所需的投递与观测操作。它不提供通用 `tools/call`
转发器，也不为身份恢复新增一套宿主 service-token 框架。缺真实来源、未登记目标
和宿主拒绝应在对应边界显式返回。

## 正常服务生命周期

```mermaid
flowchart TD
  S([启动受管 bridge]):::boundary --> A[读取宿主能力并初始化]:::action
  A --> B[(发布自有本地入口)]:::data
  B --> C[真实会话绑定后发布当前能力并处理请求]:::action
  C --> D[响应正常停止或上游断开]:::action
  D --> E([清理自有入口并退出]):::boundary
  classDef boundary fill:#E9EEF5,stroke:#64748B,color:#0F172A,stroke-width:1.5px
  classDef action fill:#E8F1FF,stroke:#2563EB,color:#0F172A,stroke-width:1.5px
  classDef data fill:#F1EAFE,stroke:#7C3AED,color:#0F172A,stroke-width:1.5px
```

运行中的多请求由服务事件循环处理。图不为轮询或重试造回边。
stdin 结束、正常信号或上游关闭时，服务退出或明确失效。只清理自有文件和 socket，
不逃离宿主管理，不忽略终止，不拥有 Collab daemon 或 tmux 的生命周期。
正式 runtime 目录位于用户私有持久根下，不把生产 UDS 固定到临时目录。

机器图：[服务生命周期](../dagpipe/collab-desktop-bridge-service.graph.json)。

## CLI 当前能力发现

bridge 在真实 executor enrollment 后，用原绑定读取 native thread 的实际 cwd。
以 canonical cwd、agent sessionID、native thread ID 三项 scope 发布当前能力。
sessionID 锚定身份，另外两项限制本次通信范围，不创建第二套身份注册表。

能力声明位于 `~/.collab/runtime/desktop/capabilities/v1/`。路径依次为 `cwd/`、
cwd 的 UTF-8 小写 hex、`session/`、sessionID 的 hex、`thread/`、thread ID 的 hex，
每个编码值按最多 120 字符分段，末尾为 `current.json`。这是可逆精确索引，不是
socket 扫描或历史身份推断。文件仅包含 schema 与既有 `desktop_bridge` candidate，
不包含名字、角色、token、caller turn、时间戳或 verified 标记。

发布者先写好本实例独占的不可变文件，再在同目录建立临时硬链接，并原子 rename
为 `current.json`。同 scope 后一次成功发布替换前者。退出时仅删除本实例独占文件，
不删除共享 `current.json`，因此旧进程不能删掉后继能力，也没有崩溃后遗留的目录锁。
每个 scope 可留一份失活声明；新发布直接替换。崩溃遗留文件不授予能力、不阻止
后继发布；本增量不承诺自动回收所有异常退出的孤立文件。

CLI 在 mutating context 准备操作前按当前 scope 读取一次声明；显式提供 transport
时沿原路径，query 和普通身份读取不发现能力。CLI 核对私有控制目录、常规文件、
64 KiB 大小上限、schema、scope 和 endpoint 范围，不跟随符号链接。然后把候选交给
daemon 原有 adapter 验证，不重复 probe，不修改 daemon 协议。错误保留身份和工作，
不转投旧 socket 或 AppServer，不要求 agent 调 MCP。

Desktop 环境标记可能被 tmux 子进程继承。声明读取失败时，CLI 复用既有 ancestry
observer 核对真实当前 pane；证实存在时让 daemon 继续原有 tmux 优先 admission。
不凭 `TMUX_PANE` 字符串认定在场，也不让未选用 Desktop 字段挡住可用 pane。

首次启动而没有真实 executor enrollment 的 bridge 不能发布这份能力。CLI 不从
session/thread 环境变量编造 turn。该场景返回明确的 Desktop 能力缺失；已登记的
当前能力发现与全新聊天的无人介入首次绑定分别验收。

## 一次消息投递

```mermaid
flowchart TD
  S([daemon 已持久化的消息]):::boundary --> A[读取当前收件路由]:::action
  A --> B[核对桥接实例与绑定目标]:::action
  B --> C[通过本地入口调用 Desktop]:::action
  C --> D[分类接受、拒绝或结果未知]:::action
  D --> E[(记录独立投递回执)]:::data
  E --> F([返回本次投递结果]):::boundary
  classDef boundary fill:#E9EEF5,stroke:#64748B,color:#0F172A,stroke-width:1.5px
  classDef action fill:#E8F1FF,stroke:#2563EB,color:#0F172A,stroke-width:1.5px
  classDef data fill:#F1EAFE,stroke:#7C3AED,color:#0F172A,stroke-width:1.5px
```

所有结果汇入一个 typed 出口。只有明确的官方成功回执才能标为 transport 接受。
请求发出后超时或断开保留结果未知，不自动重发，不切换 AppServer、tmux 或旧 socket。
socket 可连接不等于聊天在线；busy/idle 读取实际原生 thread 状态。

transport 接受、实际新 turn、`recv` 消费和回复是不同事实。消费继续沿
[既有消费图](../dagpipe/collab-notification-consumption.graph.json)，不加入 bridge ACK。
机器图：[Desktop 投递](../dagpipe/collab-desktop-bridge-delivery.graph.json)。

## 验收边界

正式 installer 安装 `~/.cargo/bin/collab-desktop-mcp`。Desktop 的 Collab MCP
入口使用这个正常 stdio launcher，由 Desktop 提供 `CODEX_APP_TOOLS_PIPE_PATH`。
launcher 优先使用宿主 `CODEX_MCP_NODE_PATH`，缺省使用 PATH 中的 Node。配置示例：

```toml
[mcp_servers.collab]
command = "/Users/your-user/.cargo/bin/collab-desktop-mcp"
env_vars = ["CODEX_APP_TOOLS_PIPE_PATH", "CODEX_MCP_NODE_PATH", "CODEX_SESSION_ID", "CODEX_INTERNAL_ORIGINATOR_OVERRIDE"]
```

将示例中的 home 换为实际安装目录。`env_vars` 传递宿主当前提供的值，不能省略。
不要手填 tools pipe、caller thread 或 turn。
这是 bridge 服务的一种正常加载方式，不要求 agent 用 MCP 工具注册或发信。
MCP `collab_context` 可从该次 executor metadata 注入新 bridge；普通 CLI 自动读取
该 bridge 当前发布的受限能力，详见[CLI 用法](../../collab/skills/collab/references/migration-daemon.md#desktop-cli-registration-and-messaging)。
Desktop 身份锚点始终是 agent sessionID，thread 和 socket 不选择身份。
CLI 不拥有 Desktop MCP 的启动，也不会为 CLI 调用另起后台 bridge。bridge 服务
必须先具备真实 enrollment；无此能力的首次绑定不因发现逻辑而自动成立。

先保留旧候选在 Desktop candidate 上失败的公开入口红测，再验证新分支。
组合验收必须从正式 `context` 和 `collab sendmessage` 进入，取得 durable msg ID、
独立通知结果、Desktop 新 turn、实际消费与回复。停止自有 bridge 后，通知应明确
失败，消息与身份保持；未登记目标和伪造 caller 输入不能到达其他聊天。

源码、候选 fake-host 测试、正式安装、当前 MCP/daemon 加载及真实 Desktop
结果分别报告。临时实验的成功不能替代正式 Collab 组合验收。
