# 当前身份映射、名字和恢复

安装二进制、安装 Skill、激活 daemon 是三个状态。只有当前 daemon 的公开入口验收
才能证明新版正在运行。向 master 或 peer 发恢复通知不等于完成版本切换。

## 唯一身份源

Collab 用 `~/.collab/identity-map.json` 保存当前名字、角色、agent sessionID、宿主
类型及投递绑定。文件格式是 `schema_version`、`slots`、`effects` 和
`pending_resets`；`effects` 只保存尚未完成的当前效果，完成后删除。
注册、换绑、改名、角色变化、恢复和注销都经过这个 owner。

daemon 的消息、任务、有效授权和关系保存在各业务 owner 的当前 `state.json`。
它们不能反向选择身份。所有 journal 读写、重放、修复、压缩、阶段记录和旧注册
挑战链均已移除。不要生成替代历史，不要从档案、旧事件或操作回执选身份。

旧版本的当前数据由本次获授权的一次性交接准备。新版没有旧表 import 或 journal
读取入口。已有业务存储缺少新当前状态时返回迁移错误，不会静默置空。
agent 不手改身份表、复制 token、reset 项目或替换旧 worker 来绕过错误。

## 正常恢复

在实际项目和实际宿主执行：

```sh
collab context
```

`cwd` 选择项目。daemon 只在当前 map 内查询本项目和 app scope 的绑定。
当前名字、agent sessionID 或 manifest 固定 binding 定位同一当前对象；旧数据
不能否决本次有效注册或用户授权。没有映射时按本次真实事实注册新的当前载体。
缺事实、坏文件、不可用服务和写入失败都返回原错，不合成恢复成功。

恢复请求在原 agent 会话执行。使用该会话已提供的 shell，工作目录设为目标项目根。
以下每行以用户意图或实际回执为触发条件。任何分支都禁止 Computer Use、CUA、
桌面或浏览器控制，也禁止用这些工具寻找可执行的终端。

Rex 和 tmux 使用各自的宿主证据。Rex 会话保留原环境，直接运行无参数的
`collab context`。不要检查 tmux pane、清除 `TMUX_PANE` 或改 AppServer scope
来改变恢复结果。只有确认当前宿主是 tmux 且回执明确缺 pane 时，才执行 tmux 查询。

| 触发条件 | 命令、参数和继续条件 |
| --- | --- |
| 用户要求首次注册或恢复自己的普通 peer | `collab context`，无参数。已有合法角色随当前名字恢复。若与用户指定角色不符，报告实际角色，等待明确的角色操作授权。 |
| 用户明确授权接替 Jenny、指定名字或角色 | 用[授权表](#当前授权与角色)给 `collab context` 加 `--approve-grant '<JSON>'`。 |
| 用户只要求查看本会话当前身份 | `collab context --query`，无其他参数。缺映射时报告原结果，等待注册指令。 |
| `completed` 且 `result.snapshot.registered=true`，名字、角色、绑定均有效 | 从该 snapshot 读取当前身份、项目、master 和订阅，结束身份恢复。投递仍以实际消费回执验收。 |
| 仅显式 `--project` / `--app-scope` 的 Context 成功，或下一条 `recv` 返回 `BOARD_IDENTITY_REQUIRED` | 在执行 `recv` 的同一 shell 和 cwd 运行无参数选择的 `collab context --query`。对照其项目、app scope 和当前绑定。成功显式探查另一 scope 不证明无参数 `recv` 能读取该邮箱。缺少同作用域绑定时保留原错，在该身份的合法原环境恢复。 |
| `missing_facts` 或 `requires_identity_update` | 只读 `result.requires.fields` 和 `result.requires.sources`，按下方宿主表查对应字段。若回执仅提供旧式快照，读取 `requires_identity_update.required_fields` 和 `field_descriptions`。 |
| `scope_required` | 从 `result.requires.sources.target_app_scope_ids` 读取候选。仅在目标 scope 由本次授权明确指定时，按 `result.requires.repair_invocation` 用 `--project`、`--app-scope` 和原授权 JSON 重试。否则交授权者选定。 |
| 发送返回 `ADDRESS_NOT_FOUND` | 用 `collab list` 核对完整 `<project>.<name>`。地址缺失时记录目标没有当前持有者。若目标是 Jenny，从 Jenny 的原会话恢复她的绑定。自己的身份只有出现独立错误时才查 `collab context --query`；普通 peer 不代替 Jenny 注册、提权、切换 scope 或重发到别名。 |
| `DAEMON_UNAVAILABLE`、版本读不到或出现 `invalid project context envelope` | `collab status --check`。记录 daemon 状态、版本及原错，交服务 owner。若回执仅为 `caller_context_required`，在原会话继续 `collab context`。 |
| `IDENTITY_MAP_DECODE_FAILED`、参数冲突、空绑定、其他非成功结果 | 保存原错、原项目和受影响动作，交对应 owner。原会话不换工具猜测或重试；有新事实或修复回执后按对应行继续。 |
| 无原会话 shell 或 `collab` 不可执行 | 报告原宿主缺少的入口或命令错误。等待原宿主恢复。 |

按回执做宿主诊断。宿主类型由当前宿主声明和 context 缺项确定：

| 已有证据 | 允许的只读检查和处理 |
| --- | --- |
| 当前确认为 Rex，且 Context 缺位置，或仅有 `REX_BLOCK` 时回执请求 AppServer `endpoint` | 在原 block 按 [Rex 恢复命令](../operations.md#rex-中的-codex) 用官方 `whoami --json --autostart=false` 读当前位置。核对已有值，从 `session_id`、`block_id` 补齐当前 shell 的 Rex 环境并重试。缺字段、位置冲突或收到 `REX_EXECUTION_CALLER_MISMATCH` 时保留原错并交当前宿主修复。 |
| 当前确认为 tmux，context 缺当前 pane 事实 | 仅在 `TMUX_PANE` 已由本会话注入时执行 `tmux display-message -p -t "$TMUX_PANE" '#{session_id} #{pane_id} #{pane_tty}'`。遇到 `TMUX_OBSERVATION_CAPABILITY_MISSING` 或 `TMUX_ITERM2_PANE_REQUIRED`，交宿主维护者；由 daemon 验证现有接入。 |
| Desktop 回报 `DESKTOP_BRIDGE_SESSION_REQUIRED`、`DESKTOP_BRIDGE_THREAD_REQUIRED` 或其他 bridge 缺项 | 在当前 Desktop shell 核对 `CODEX_SESSION_ID`、`CODEX_THREAD_ID` 是否由宿主提供。缺值或能力交 Desktop 宿主补齐。标准 Desktop 由 `collab context` 自动取得 bridge 能力。 |
| DSH context 返回 session 或 gateway 缺项 | 在原 DSH shell 核对 `DSH_SESSION_ID`。由 gateway 核实当前会话和项目，缺控制能力交 DSH 宿主。 |
| 已知为直连 AppServer 且回执明确请求字段 | 按下表从当前宿主取值并用 `--provide` 提交。参数只包含回执要求且已核实的字段。 |

宿主补齐事实后，回到发生错误的原会话执行 `collab context`。恢复后的工作交接另走
[Board 操作](../operations.md#public-task-board-and-read-only-dashboard)。例如“恢复 peer、向 Jenny 报空闲、接替 Carol 的工作”，
先恢复当前 peer。从成功快照确认当前 master 和任务，只有在已获发送授权且快照表明确实
空闲时，才用 `collab send --to '<当前项目>.Jenny' --subject '空闲' --content '<实际状态>'`。
收到发给自己的任务邀请时，从当前 Board 视图取 `id` 和 `revision`，执行
`collab board respond '<id>' --accept --expected-revision <revision>`。接替工作采用任务
邀请或当前任务已有的认领入口，保留自己的身份。

`collab context` 返回 `missing_facts` 时，只补 `result.requires.fields`
列出的字段。先按 `result.requires.sources` 查询当前宿主。同样的信息如果已经
由宿主注入，直接重跑 `collab context`。各宿主的取值命令见
[宿主注册所需信息](../operations.md#宿主注册所需信息)。

| 字段 | 从哪里取 |
| --- | --- |
| `session_id` | 当前 agent shell 的 `CODEX_SESSION_ID`。DSH 走独立路径，由 gateway 注入 `DSH_SESSION_ID` 并核实。 |
| `thread_id` | 当前 Codex shell 的 `CODEX_THREAD_ID`。Rex 和 tmux 注册按各自宿主步骤取值。 |
| `endpoint` | 直连 AppServer 时取当前宿主注入的 `COLLAB_APPSERVER_SOCKET` 或 `CODEX_APP_SERVER_SOCKET`。若值为绝对 socket 路径，加 `unix://` 前缀。标准 Desktop 由 CLI 请求 bridge 能力，通常无需填此字段。 |
| `namespace` | 直连 AppServer 时由当前宿主标记派生。Codex Desktop 为 `codex_app`，Codex CLI/TUI 为 `codex_tui`。非标准集成使用宿主明确提供的 `COLLAB_APPSERVER_NAMESPACE`。 |

例如，非标准直连 AppServer 的回执若只要求 `endpoint`，且当前宿主已给出
该 socket，就提交一个只含该字段的 JSON 对象：

```sh
collab context --provide '{"endpoint":"unix:///实际宿主提供的绝对路径.sock"}'
```

不能把 agent sessionID、native thread ID 和 tmux `$N` 互相替代。
已经命中当前映射时可复用该映射的 adaptor；复用不表示观测到了新的端点。
不要从旧 transcript 抄信息，不猜 socket，也不把 MCP tools pipe 当 AppServer。

`collab context --query` 只读当前映射和当前快照，不查旧操作，也不需要操作能力码。
`--op` 仅关联本次调用，不建立操作历史或恢复 selector。

daemon 确认身份丢失后的十五分钟只约束自动重派。等待期内 manager 可以按现有合法
邀请入口提前邀请。邀请不转任务归属。目标接受且 daemon 提交后，任务归属才转移。

## 当前授权与角色

已有合法角色随名字及唯一当前绑定自动恢复。新的 master 授权需要本次人类明确指令。
普通 peer 恢复保持其当前合法角色。项目 master 统一使用 Jenny 名字。
在要接替的 agent 会话中、从项目根目录执行。项目由当前 cwd 选定，app scope
沿当前宿主配置。普通注册和续接先直接用 `collab context`。

```sh
collab context --approve-grant '{"action":"restore_master","authorization":"<本次用户授权>"}'
collab context --approve-grant '{"action":"restore_peer","authorization":"<本次用户授权>"}'
collab context --approve-grant '{"action":"restore_name","requested_name":"Alice","authorization":"<本次用户授权>"}'
```

`action` 按用户本次指令选择。接回 Jenny/master 用 `restore_master`，
daemon 会选 Jenny，无需 `requested_name`。`restore_name` 的
`requested_name` 填用户本次明确指定的名字。把 `authorization` 的占位文本
替换成用户本次明确授权的原话，例如“我授权当前 AppSDK 会话接管 Jenny/master”。
没有这项授权时沿普通注册继续。授权 JSON 不需要填写 session、thread 或 socket。

授权作用于本次 map 选定的当前对象。不要提交旧 `target_identity`、incumbent fence、
digest、generation、时间戳、裁决请求或旧完整 approval。旧身份准入和 grant
replacement 阶段链已删除。当前 scope 不符或授权缺失会明确失败。

正常成功结果是 `completed`，且 snapshot 的 `registered=true`。读取其中的实际
名字、角色和当前绑定继续工作。pending、queued、missing_facts 和错误都不是恢复
完成。只补返回结果要求的真实事实，不让 agent 选择历史身份。

人类明确退役 Jenny 时，释放 Jenny 当前绑定并撤销该精确 holder 的 master grant。
未完成任务立即具备重派资格。消息、资源、工作记录和恢复入口保留。Jenny 继续作为
master 专用名字。普通注册不得自动取得 Jenny。E3 执行验收尚未交回。

## 固定布局与物理名字

Collab 和 tm-iterm 共用 `~/.local/state/tm-iterm/manifest.json`。manifest 管理固定
`binding_id`、`tab_id`、`pane_order`、cwd 和 iTerm profile。Collab 只关联固定 binding，
不改布局位置。正常运行由 Collab 改名；tm-iterm 默认创建和解析，只有用户显式
调用批量改名脚本时才改名。

物理 tmux session 名是 `<cwd末级folder>_<固定位置序号>_<Collab名字>`，例如
`routecodex_1_Jenny`。序号始终属于 manifest 固定位置，不能随角色或名字分配。
只有固定位置 1 可以升为 Jenny。排序和 pane 对应读取 manifest 的 `tab_id` 与
`pane_order`，不能从 session 名格式猜布局。Collab 后缀不改变固定 binding。

tm-iterm 只读 Collab identity map。禁止把转换出的 `entries`、布局数据或迁移预览
写入 `~/.collab/identity-map.json`。布局 manifest 与身份表有各自的唯一 owner。

该固定位置 gate 作用于 tmux。Rex 使用当前 server/session/block，按唯一当前 block 续接。
项目 Rex session 按 cwd 末级目录命名，身份、角色和任务变化保留该名称。

Desktop 和 DSH 使用同一身份表，只是绑定方式不同。Desktop 使用宿主 bridge 的
当前 sessionID 和真实 caller 能力；DSH 使用 gateway 的当前 sessionID、endpoint
和接收循环。两者不依赖 tmux 改名，也不增加不存在的 native 字段。

名字续接同步该名字下未关闭任务的当前 sessionID，保留任务 ID、证据、资源与状态。
名字交接转交未消费消息，并对齐当前收件和唤醒目标。已消费消息不重放，不伪造
消费结果。队列接受、宿主接受和真正消费分别验收。

任何身份、升级或通信恢复都不关闭、重启 tmux server，也不终止宿主 pane。
