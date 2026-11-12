# Rex 与 Collab 适配 Ontology

状态：current behavior design。本文记录现有行为的对象、状态、关系和
可执行 DAG，不新增全局规则，也不把设计文件当作运行证据。

## 边界

Rex terminal endpoint 是通信地址。Collab agent sessionID 是身份执行载体。
两者不能互相替代。Rex 的输入排队、snapshot 读取和程序状态观察分别有独立
语义；任何一项都不证明消息已接收、消费或回复。

## 对象

| 对象 | 身份 | 关键属性 | 唯一 owner | 可观察入口 |
| --- | --- | --- | --- | --- |
| Rex endpoint | server + service generation + session + block | host、instance、cwd | binding owner 与 Rex adaptor | 官方 CLI 参数、typed binding |
| 通知提交 | 一次通知消息对应一次正文 write 加一次真实 Enter 按键 | 原始正文、排队字节、Enter 回执 | `collab/src/adapters/rex.rs` | write receipt、send-key invocation |
| 临时 snapshot | endpoint + 请求行数 + 一次 capture | 最新文本行、传输类型、非持久 | Rex adaptor 与 snapshot handler | `collab snapshot TARGET_ID` |
| 程序状态记录 | 一条 Rex program-status record | id、state、kind、owner、updated_at | adaptor 解码，monitor 投影 | `program_status`、activity view |
| foreground owner | 当前前台进程 | pid、name | 官方 `terminal.process` | process query |
| 通知拒绝 | selected transport + 原始错误 | transport 标签、native detail | `notification_rejected_label` | 通知响应 |

## 行为

### 通知提交

`（已选择 endpoint，通知正文） -> （write 排队正文，send-key Enter 提交，不消费）`

- 正文按 UTF-8 原样保留，write 不追加 `0x0D`。真实 Codex TUI 会把正文
  内的 CR 当作文本保留，不作为提交键；提交必须来自独立 Enter 按键事件。
- write 返回只表示字节已排队。Enter 前有短暂有限等待，让目标终端先消费
  正文，避免提交键在正文前丢失。
- Enter 失败时保留 `WriteQueued` 阶段并报告 unknown，不重发正文，也不把
  输入提交冒充消费。

### Snapshot 读取

`（已鉴权目标，1..200 行） -> （最新 N 行文本 snapshot，无输入副作用）`

- 只调用官方 `capture`，使用 text、`--unwrap --trim`。
- 返回空文本合法；失败不回退为空快照。
- 不修改 identity、presence、activity、消息、订阅或消费回执。

### 活动投影

`（当前状态记录，当前 foreground owner） -> （busy|idle|waiting_user|unknown）`

- owner 的 pid 和 name 必须同时匹配当前 foreground。
- 过期或不完整 owner 的状态不参与活动判定。
- 有效 `blocked` 优先映射 waiting_user；有效 `working` 映射 busy。
- 有效 `idle`、`done`、`error`、未知或缺失 state 映射 idle。
- 无有效记录、查询失败或证据缺口保持 unknown。

### 拒绝标签

`（selected transport，底层错误） -> （REX_NOTIFICATION_REJECTED: 原错）`

Rex 标签只由通知结果映射 owner 生成，adapter 和 CLI 不重复包装。

## 关系

| 关系 | 方向 | 基数 | 约束 |
| --- | --- | --- | --- |
| endpoint 承载通知提交 | endpoint -> submission | 1:N | submission 只写已核实 endpoint |
| endpoint 支持 snapshot | endpoint -> snapshot | 1:N | snapshot 只读 |
| 状态记录属于 foreground | foreground -> status record | 1:N | pid/name 同时匹配才有效 |
| 通知提交产生拒绝 | submission -> rejection | 0..1 | 拒绝保留原错 |
| Collab 身份使用 endpoint | identity binding -> endpoint | 0..1 current | 不因 Rex 观察失败重建身份 |

## 通知提交图

```mermaid
flowchart TD
  S([收到 Rex 通知]):::boundary --> M{提交模式允许?}:::decision
  M -->|否| NO([明确拒绝，不写终端]):::failure
  M -->|是| V[核实服务代际与目标]:::action
  V -->|失败| NO
  V -->|通过| W[写入正文]:::action
  W -->|完整排队| E[发送真实 Enter]:::action
  W -->|拒绝| NO
  W -->|未知或不完整| UNK([保留 unknown]):::failure
  E -->|已接受| OK([返回正文与 Enter 已接受，未消费]):::success
  E -->|拒绝或未知| UNK

  classDef boundary fill:#E9EEF5,stroke:#64748B,color:#0F172A,stroke-width:1.5px
  classDef action fill:#E8F1FF,stroke:#2563EB,color:#0F172A,stroke-width:1.5px
  classDef decision fill:#FFF4CC,stroke:#D97706,color:#0F172A,stroke-width:1.5px
  classDef success fill:#E8F7EE,stroke:#16A34A,color:#0F172A,stroke-width:1.5px
  classDef failure fill:#FDECEC,stroke:#DC2626,color:#0F172A,stroke-width:1.5px
```

## Snapshot 图

```mermaid
flowchart TD
  S([收到 snapshot 请求]):::boundary --> A{鉴权与行数有效?}:::decision
  A -->|否| NO([明确拒绝，不读取]):::failure
  A -->|是| V[核实服务代际与终端目标]:::action
  V -->|失败| NO
  V -->|通过| C[官方 capture 读取终端]:::action
  C -->|成功| T[(最新 N 行临时文本)]:::data
  T --> OK([返回非持久 snapshot]):::success
  C -->|失败| NO

  classDef boundary fill:#E9EEF5,stroke:#64748B,color:#0F172A,stroke-width:1.5px
  classDef action fill:#E8F1FF,stroke:#2563EB,color:#0F172A,stroke-width:1.5px
  classDef decision fill:#FFF4CC,stroke:#D97706,color:#0F172A,stroke-width:1.5px
  classDef data fill:#F1EAFE,stroke:#7C3AED,color:#0F172A,stroke-width:1.5px
  classDef success fill:#E8F7EE,stroke:#16A34A,color:#0F172A,stroke-width:1.5px
  classDef failure fill:#FDECEC,stroke:#DC2626,color:#0F172A,stroke-width:1.5px
```

## 活动状态图

```mermaid
flowchart TD
  S([读取 Rex 当前事实]):::boundary --> Q{查询完整?}:::decision
  Q -->|否| UNK([unknown]):::failure
  Q -->|是| O[比较记录 owner 与当前前台进程]:::action
  O --> V{存在有效记录?}:::decision
  V -->|否| UNK
  V -->|是| B{有效 blocked?}:::decision
  B -->|是| WAIT([waiting_user]):::success
  B -->|否| W{有效 working?}:::decision
  W -->|是| BUSY([busy]):::success
  W -->|否| IDLE([idle]):::success

  classDef boundary fill:#E9EEF5,stroke:#64748B,color:#0F172A,stroke-width:1.5px
  classDef action fill:#E8F1FF,stroke:#2563EB,color:#0F172A,stroke-width:1.5px
  classDef decision fill:#FFF4CC,stroke:#D97706,color:#0F172A,stroke-width:1.5px
  classDef success fill:#E8F7EE,stroke:#16A34A,color:#0F172A,stroke-width:1.5px
  classDef failure fill:#FDECEC,stroke:#DC2626,color:#0F172A,stroke-width:1.5px
```

## 执行 DAG

可执行对象流由
[`collab-rex-adaptor.graph.json`](../dagpipe/collab-rex-adaptor.graph.json)
维护。该图保持单入口、单出口和线性 SESE；本 ontology 解释节点语义，
DAGPipe CLI 只校验拓扑与绑定，不证明运行行为。
