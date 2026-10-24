---
name: collab
description: "仅约束已成功注册 Collab 的 peer；master 权限需另行授权。未注册、注册失败或无法注册时独立完成，不适用协作协议。用户明确要求注册时才初始化。"
---

# Collab

## 通信入口与项目目录

普通消息默认走公开 CLI。使用成功注册时的项目根目录；业务代码可以在独占
worktree 中修改，但不要为查身份或发消息切到 `~`、临时目录或收件人的项目。
已关联的同项目 worktree 可由 Collab 解析；目录不确定时，回到已知注册项目根目录。
已有身份和收件人时直接发送，不先重跑 context、收件或通信恢复。
发送不等待对方空闲、上一条消息消费或回复；不要先轮询消息状态再发送下一条。
daemon、DSH gateway 和 Desktop bridge 立即提交新消息，接受、消费与回复分别记录。
`collab context`、`collab inbox` 和 `collab sendmessage` 都是公开 CLI，
不是私有命令。内部控制协议与公开 CLI 的边界不同，不要把 shell 调用本身视为违规。
同项目的 Alice、Jenny 等已注册名字就是收件地址，不需要额外的聊天、thread、
teammate 或 worker 标识；不知道这些内部标识不构成转请 master 代发的理由。

| 操作 | 使用入口 |
| --- | --- |
| 首次注册或明确恢复身份 | 在自己的项目运行 `collab context` |
| 同项目发送或通知 reviewer | `collab sendmessage --to '<稳定名字或真实from>' --subject '<主题>' '<正文>'`；例如同项目 Alice 可直接用 `--to Alice`，不需要先找内部 worker ID 或让 master 转发 |
| 回复原请求 | 同一发送命令，原消息确有 ID 时加 `--in-reply-to '<原消息ID>'`；没有 ID 不猜、不查内部信箱找值 |
| DSH 收件 | gateway 将完整正文送入原生回合；直接处理和回信，不运行 `recv/ack` |
| 非 DSH 收件 | 按本 Skill 的 `collab recv` 流程消费 |
| 确实跨项目 | 沿下文 master-to-master 通道；普通 peer 交给自己的 master |

需要显式固定目录时，整条命令是：
`(cd '<已注册项目根目录>' && collab sendmessage --to 'Alice' --subject '<主题>' '<正文>')`。
CLI 正文是位置参数，不是 `--body`。DSH 若暴露 `collab_send`，它也是公开的宿主
发送工具，JSON `body` 属于该工具参数；不要与 CLI 参数或内部控制协议混用。
不要因一次错误在 CLI、MCP、宿主工具和内部 socket 之间试探切换。

`context` 和 `inbox` 中的身份拒绝首先涉及当前发件人及其项目，不能据此认定
Alice 等收件人不存在。若 `resolve-agent` 错误列出自己的 session 和错误目录
（例如 `/Users/…`），先回已注册项目目录，使用同一公开入口；不要重新注册、
查 journal/route/token，或调用 `register-peer`、`resolve-agent`、`Poll/Ack` 修身份。
在正确项目仍失败时，保留原命令、项目、错误和已有消息 ID，报告具体失败边界。

## 协作请求的执行、回复与接续

收到通知后读取完整原消息，区分请求中的 action、回复要求和普通信息。有已授权 action
就实际落实并向原请求方回复结果、证据或具体阻塞；不要只读通知、ACK，或只向用户总结。
无 action 但要求答复时，直接回答原问题；仅普通信息且未要求回复时，不自动回“收到”。
“完成后回复”要求在交回实际结果时答复，不能用“已收到、正在做”代替完成结果。

先区分项目内与跨项目消息。下面的 `sendmessage` 回复命令仅用于项目内消息。
跨项目消息的 `from` 是 `<worker-id>@<source-project>`；必须按下文
“Cross-project master communication”走 `master send`，不能把整个跨项目地址传给 `sendmessage`。
项目内回复使用原消息的真实 from 和 id，不猜名字或任务 ID：
`collab sendmessage --to '<原消息from>' --subject '<主题>' --in-reply-to '<原消息ID>' '<实际结果、答复或阻塞证据>'`。
向用户报告、消费通知、ACK 或写任务进度都不等于已回复请求方。回复不自动要求对方再回，
只有新的 action 或明确追问才开启新一轮请求，避免无意义互答。
系统提醒本身不是请求方；它引用原消息时回复该原消息的发送者，不向 `collab-server` 代答。

项目内普通投递使用以下参数；两个回复选项互斥，不带选项默认不建立 daemon 回复标记：

```sh
collab sendmessage --to '<接收者>' --subject '<问题>' --reply-required '<需要答复的问题或action>'
collab sendmessage --to '<接收者>' --subject '<任务>' --reply-on-completion '<已确认范围与交付要求>'
collab sendmessage --to '<原消息from>' --subject '<结果>' --in-reply-to '<原消息ID>' '<实际答复>'
```

选项放在位置正文之前。结果回复不附加回复要求；消费、ACK 或对用户汇报不清标记，
真正关联回复的成功入队才清标记。若只报中间进度且仍需完成后答复，发送不带
`--in-reply-to` 的普通进度消息，不提前消除原回复要求。daemon 不判断正文是否真的完成。
这些参数属于项目内 `sendmessage`；跨项目 `master send` 尚未接入回复标记。

交回协作结果后，立即接续自己原来的未完任务。先核对待回复要求、协作请求和本人任务，
落实当前可执行动作；确无可继续工作时才 idle。完成后回复尚待外部条件时保留标记并说明
等待条件，不能伪造完成答复；也不能因为一个协作请求已交回就结束自己的工作。
收到 daemon 待回复提醒时，读取其引用的原消息，按原回复要求处理，不把提醒再当新任务，
不对提醒本身回“收到”。daemon 仅追踪待回复标记，不替 agent 生成业务答复或判定任务完成。

peer 默认向请求方、委托者或所属 master 回报，不向用户输出例行进度或结案报告。
用户直接提问、明确要求报告、必须由用户决定或影响用户的重大异常时，才向用户说明。

## 工作角色与用户指定分工

工作角色是稳定名字的专长，例如 `designer`、`reviewer`。同一 peer 可有多个专长。
它们不改变 master/peer 职责，也不增加项目权限。省略 `--work-roles` 保留原值，
明确传空字符串清空。当前 master 可按用户指令为其他稳定名字设置角色。

先判断用户要求：注册/恢复协作身份、设置工作角色、指派具体任务是三种独立操作，
不是每次都要依次完成的检查表。“担任 iOS app 执行负责人”先按用户确认保存工作角色；
除非同时明确了任务或某类工作的默认分工，不创建 board 任务，不写 `board policy`。
设置自己的角色不要求 master 代办，也不以任务 ID、board 可用或已有执行者为前提。

### 指派前确认并保存

master 或负责子目标的 peer 在指派前先分析需求，向用户列明 scope（目标、允许修改的
范围、非目标、交付与验收条件）和角色要求（工作类别、角色偏好、指定执行者、指定
reviewer）。用户确认后才保存分工并发出协作请求。用户已经明确指定或确认的条目
直接复用，只确认缺失或有歧义的部分；未确认前可做只读分析，不派发受影响的执行。
未指定固定执行者或 reviewer 是允许的，不将可选字段为空当作必须补问的缺口。

角色用 `collab context --work-roles` 按项目和稳定名字持久保存；分工用 `collab board policy` 与
任务明确字段保存。scope、交付与测试条件写入已有 board 任务的 description、
delivery-condition、test-condition，用户确认原文或所在对话位置记入该任务描述。
角色及执行者约束以正式控制字段为准，不能只留在聊天、消息正文或 agent 临时记忆。
不新增第二份角色表或审批状态。保存后读取任务回执，确认范围和有效分工符合用户要求。

同一已确认 scope 内的后续请求直接沿用这些要求，不逐次请用户重复确认。改变 scope、
指定执行者、reviewer 或角色要求时，先向用户说明差异并确认，再更新受影响任务与分工。
普通进度更新、重新唤醒、换会话不构成重新确认的理由。

工作角色可动态新增、修改或撤销。用户明确提出的角色变更就是该次确认，不重复请示。
`context --work-roles` 设置完整角色集合：新增时保留仍需要的旧角色，删除时省去对应
角色，全部撤销时传空字符串。修改后读取回执，后续分派和每次接单读取最新持久角色，
不缓存注册时的旧角色。角色更新不自动改写已有任务的 scope、指定执行者或 reviewer；
需要同步变更任务时，按用户确认更新对应任务约束。不能凭角色变化自行接管或放弃任务。

### 每次接收协作请求

仅具体执行、交付或审查请求需要任务 ID。注册/恢复身份、更新工作角色、需求讨论和
状态查询不要求先有 board 任务；不要为这些请求虚构或补建任务。
master 与 peer 收到具体执行请求，在接受邀请或开始执行前，读取当前任务，
核对已确认 scope、有效角色要求、指定执行者、reviewer、交付与验收条件。恢复会话时
通过 `collab context` 读取持久身份与角色，不从旧消息或会话身份猜测工作归属。
具体执行请求缺少任务或必要条件时，由指派者补齐，只暂停受影响的执行。

请求符合已确认要求时，按原约束接受并编排执行；角色偏好不等于执行授权。请求与
保存的约束冲突或超出 scope 时，向指派者报告具体差异，由其取得用户确认并更新任务，
接收者不自行放宽要求。向下派发也必须传递任务 ID、scope、角色与审查要求；每次交付
按同一任务的当前条件验收，并送指定 reviewer 审查，不能以消息已读或已接受代替完成。

### 按当前请求选择命令

在用户指定的项目 cwd 中执行。下列命令是不同场景的入口，不是连续执行脚本。
`<...>` 必须替换为用户确认值或真实回执，不能直接照抄占位符。

| 当前请求 | 直接执行的命令 | 到此为止的条件 |
|---|---|---|
| 用户要求注册/恢复，或当前身份确实缺失 | `collab context` | 读取 registered、identity、work_roles、role_brief、operations。已有当前有效回执就复用；仅按返回的缺失事实通过 `context --provide '<真实事实JSON>'` 补充，不猜宿主事实。 |
| 设置或修改自己的工作角色 | `collab context --work-roles '<完整角色集合，逗号分隔>'` | 检查 work_roles_update 回执；保留仍需的原角色。成功即完成角色设置，不继续创建任务或设置执行者。 |
| 撤销自己的全部工作角色 | `collab context --work-roles ''` | 仅在用户明确要求清空时执行，不用作排障试探。 |
| 当前 master 按用户指令给别人设置角色 | `collab context --for-name '<稳定名字>' --work-roles '<完整角色集合>'` | `--for-name` 是设置目标名字的专长，不是切换调用者身份。 |
| 当前 master 保存用户明确要求的某类工作默认分工 | `collab board policy --scope project --work-kind '<类别>' --prefer-role '<角色>'` | 只有明确要求默认分工才使用；指定人时用 `--executor '<名字>'`，指定审查者时加 `--reviewer '<名字>'`，不补造用户未要求的人员或 reviewer。 |
| 查具体任务的当前条件、邀请或 revision | `collab board show` | 从当前回执读取任务 ID、scope、有效分工和 revision；不能把消息中的旧 revision 当当前值。 |
| 发布已确认的具体任务 | `collab board publish '<任务ID>' --title '<标题>' --description '<scope与确认来源>' --delivery-condition '<交付条件>' --test-condition '<验收条件>'` | 仅按已确认要求附加 `--work-kind`、`--prefer-role`、`--executor`、`--reviewer`；不因发布任务而强制创建项目默认。 |
| 指派者按任务有效分工发邀请 | `collab board dispatch '<任务ID>' --expected-revision <当前revision>` | 使用 publish 或 board show 的真实回执；不写死为 1。邀请不等于接受。 |
| 接收者接受发给自己的邀请 | `collab board respond '<任务ID>' --accept --expected-revision <当前revision>` | 核对 scope、约束与资源后执行；成功后才转移责任。 |
| 接收者主动认领允许认领的待办 | `collab board claim '<任务ID>' --expected-revision <当前revision>` | 遵守现有认领门禁；有工作角色不等于已有执行授权，也不要求所有合法认领必须先收到邀请。 |
| 指定 reviewer 接受交付 | `collab task review '<任务ID>' --accept --evidence '<实际审查证据>'` | 必须实际完成审查；角色标签、读消息和接受邀请不等于审查通过。 |

例如用户只要求“你担任 iOS app 执行负责人”，且已确认当前角色为空，执行
`collab context --work-roles 'iOS app执行负责人'`；不接着运行 `board policy`。
回执会规范化角色标签的大小写，以实际回执为准。

### 命令失败时的停止点

身份注册、角色保存、任务分派分别报告结果，不能把前一步成功说成全部成功。
遇到 `BOARD_IDENTITY_REQUIRED` 等错误，保留原命令、错误与已有成功回执；若没有新的
缺失事实或环境变化，不重复相同命令，不从 CLI 换 MCP 或换参数格式碰运气，不让
master 代写来绕过当前身份失败。只读一次 `collab --version` 可记录版本；无需读源码、
二进制字符串或 journal 来完成普通角色设置，不编辑身份、route、token 或控制状态文件。

只暂停失败写入及真正依赖它的后续动作。已有明确授权、scope 和有效任务归属且不依赖
该失败写入的工作可继续；没有任务就报告“角色保存失败，尚未指派具体任务”，不虚构工作。
需要产品排障时单独提出具体问题，只有用户明确授权该维修范围后才进入开发流程。
不在普通角色设置中自行安装、重启 daemon 或把修客户端列为用户必须完成的初始化步骤。

用户在 master 任务中指派某类工作时，master 将要求保存为明确分工字段。
项目默认、目标默认、任务明确字段逐项覆盖。目标使用已有 board 任务，子任务用
`--parent-task` 引用。已保存的任务约束不随默认规则变化而自动改变。
`board policy --scope goal --id <目标任务>` 保存目标默认；`--scope task` 修改指定任务，
两者使用当前 `--expected-revision`。清除约束使用 `--clear-executor`、
`--clear-reviewer` 或 `--clear-prefer-role`，不能用空名字猜测清除意图。

明确指定执行者时，只能邀请该名字当前持有者；不可用就报告等待，不自行换人。
只有角色偏好时优先选匹配者；无匹配可退选其他可用 peer，并保存理由。
邀请不等于接受，接受后才转移执行责任。peer 可编排多个目标，仍须遵守真实资源约束。

明确指定 reviewer 时，将交付交给该名字当前持有者审查。作者或 master 不能代审。
`task deliver` 表示提交待审；审查通过后仍须完成原有代码集成或只读交付流程。
重新交付或实质改变验收合同会使旧审查失效；普通进度更新和同名换会话不会。
人工终止、资源清理和验收完成分别报告，不能以 `closed` 或清理成功替代交付证据。

源码合同与三张操作图见 [工作角色与分工设计](../../docs/design/work-roles-and-assignment.md)。

## 升级交接：明确弃用 tmux session 身份规则

名字承载工作归属，agent sessionID 是当前执行载体。注册报告并绑定 agent sessionID；
同名字续接时自动更新 sessionID，并同步该名字下全部未关闭 board 任务。旧 sessionID
不是恢复门禁，不要求 Jenny 重新 assign；已关闭任务保留历史。具体合同见
[名字、会话与工作继承](references/stable-name-recovery.md)。

升级后必须摒弃旧规则“当前 tmux session 决定 peer”“按 pane/socket 恢复工作归属”。
tmux session、pane、socket 和 server PID 仅供探测与投递定位，不是身份键、名字占用键
或任务 owner。不要根据旧文档重新添加这些耦合，也不要把 tmux session ID 当作
agent sessionID。历史数据由 daemon 在正常注册或恢复时兼容，不要求 agent 手改
journal、清空 board、重建 pane 或重启 tmux。

交接给恢复者时报告当前名字、master/peer 角色、agent sessionID、未完成目标、
所持资源与需要巡检的事项。名字的当前工作者接续任务；旧执行者不另行处理。
规则升级与已加载 runtime 分别验收，不能仅凭读到新版 Skill 宣称 daemon 已升级。

master 与 peer 均自行管理本人的 longhorizon goal；旧的“goal 只归 master”
“peer 找 master 代登记”“所有 peer 忙才可发 GCM”规则已废弃。详细入口与回执沿
[本人目标合同](#register-the-long-horizon-goal-before-dispatch)。以当前正式安装 Skill
和 daemon 当前 `role_brief` 为准；旧 worktree 的 `collab/skills/` 是分发源码副本，
不能替代当前运行规则，也不能通过改写他人的 worktree 来做规则升级。

## 全局 daemon 与默认身份恢复

`collab context` 是完整恢复与旧数据迁移入口，迁移是 daemon 的内部工作。agent
不处理重复档案、旧 route 或旧身份清理。已有用户恢复/替换意图先于历史身份校验；
同一 session 的后提交绑定收敛旧绑定，不让历史歧义成为 agent 的决策分支。
准确命令、最小授权 JSON 与结果读取见
[恢复操作](references/stable-name-recovery.md#恢复操作context-自己迁移agent-不做历史清理)。

Collab 只有一个全局、由用户管理的 daemon。agent 不启动、关闭或重启它，也不为身份恢复申请维护窗口。用户维护命令、参数和凭据不属于 agent 接口。

Collab/AppSDK 与 tmux 使用独立生命周期。Collab 安装、重启、注册、身份恢复、
巡检或 daemon 不可达，都不得触发 tmux server 的重启或停止，也不得关闭、
终止宿主 pane。现有共享 Collab daemon 继续由用户管理。

默认使用 `collab context`。当前 `cwd` 决定 project，名字承载工作归属，注册报告的 agent sessionID 绑定当前执行载体；历史 pane 绑定到谁都不阻断恢复。当前 project 没有 master 时自动恢复为 peer。用户指定恢复 Jenny 时同时恢复 master；另一个在线工作者占名时，默认不替换，用户要求接管需本次二次确认。确认离线后可正常分配；归属 unknown 时交 Jenny snapshot 裁决。未获 master 授权的普通恢复继续作为 peer。不按任务内容、worktree、旧角色或 master 派单增加身份准入校验，不要求重新注册或通信回环。daemon 不可达时保留身份和任务，返回服务不可用；不启动第二个服务或修改 journal、route、credential 文件。

明确恢复授权覆盖当前 pane 的历史 peer/master；旧绑定和旧 route 自动失效，不再要求旧身份批准。agent 可以移除当前 pane 自身的 agent 绑定和 route，不关闭宿主。daemon 自动清理失效 route。当前项目 master 可以 reset 本项目的 Collab，移除历史消息、route、任务、资产和负载；只清当前项目，不操作其他项目或全局 daemon 生命周期。实际 I/O 错误保留原错，不用历史身份保护拒绝当前授权，也不伪造成功。

context 恢复还会把当前身份已存在且仍有效的订阅对齐到当前投递位置。
这不新增巡检周期，不重置触发时间、次数或到期时间；显式取消和已到期的订阅不恢复。
peer 仍负责按自己的目标创建或续订所需的巡检范围。

身份恢复不依赖 board、任务分配或空闲产能。worker 状态从所属项目的当前 binding 读取身份事实；端点和活动状态分别观察，不把 host-only route 投影缺失误报为项目身份失效。

## 适用身份

只有当前执行者已在本项目成功注册为 Collab peer，后文的协作规则才适用；各角色规则只约束该角色。master 规则还要求当前 peer 持有经明确授权的 master 权限。未注册的 `codex exec --profile gcm` worker 不因受托而成为 Collab peer，也不适用本 Skill 的协作协议。加载本 Skill、任务提到 Collab、看到 master 名称或启动多个 worker，都不代表已注册或取得 master 权限。

当前项目没有注册身份、状态为 `registered=false`，或初始化无法完成时，不需要遵从本 Skill 的协作协议：独立完成用户任务，不寻找、等待或服从 Collab master，也不因注册失败而阻塞。只有用户明确要求注册时才执行初始化；失败保留原错，并继续独立工作。成功注册后的通信故障按对应故障规则处理，不抹除已有身份或职责。

独立开发使用全局规定的外置独占 worktree。注意共享资源冲突，不改动、回收或覆盖他人的 worktree、文件、进程与 claim；能隔离就继续，不能隔离只报告受影响操作。没有 Collab 不阻断独立任务，也不要求建立一套协作生命周期。

共享任务板与 dashboard 同样只在已注册时可用：`collab board show` 需要已注册身份但不需要 daemon 生命周期动作；`collab dashboard` 只读，不注册 peer、不启动 daemon、不消费消息。

Durable identity, role, mailbox, task, and subscription truth lives in the
Collab daemon. Transport selection is decided by the daemon based on runtime
facts; initial registration prefers a verified current tmux pane. Restoration
reconciles the current explicit capability with the durable binding through
context. An explicit current Desktop bridge capability is a transport update,
not an error-driven fallback. Once a native binding is selected, tmux is never
a message, wake, presence, or status fallback. Its
observed tmux endpoint is delivery placement only. cwd selects the project,
and the stable name owns the work. The reported agent sessionID is the current
execution carrier; tmux session, pane, socket and server PID locate transport
only. Same-name session changes update all unclosed board assignments. Pane
history and old native addresses do not select or protect identity.
Desktop agents use the same CLI entry as other hosts: `collab context`,
`collab sendmessage` and `collab recv`. Calling an MCP tool is not an identity
or messaging prerequisite. Desktop's identity anchor is the current agent
sessionID; the stable name owns work. Thread IDs and bridge sockets locate
communication and never replace that anchor. A Desktop-owned bridge carries
daemon-to-chat delivery; its lifecycle is separate from the agent's choice of
CLI or MCP. See [Desktop CLI registration and messaging](references/migration-daemon.md#desktop-cli-registration-and-messaging)
for automatic current-capability discovery, commands, receipts and the
first-enrollment limit. Do not guess or reuse an AppServer socket.

DSH 的接收方式不同：gateway 独占 canonical `Poll` / `Ack`，将完整正文直接送入
原生会话。收到正文后直接处理并用 `collab sendmessage` 回复。不要运行 `collab recv`
或 `collab ack`；这两个 CLI 命令会返回 `DSH_NATIVE_DELIVERY_OWNED`，避免与 gateway
争抢信箱。需要诊断时只读 `collab msg <id>` 或 `collab inbox`。下文的 CLI 收件、
消费与 ACK 指引只适用于非 DSH 传输。通信恢复也不是处理原生正文的前置步骤。
Production projects use the globally installed Collab v1.
Candidate consumers may use the existing `COLLAB_TEST_BINARY` seam to select
the exact tested binary. Candidate results do not prove the canonical installed
binary or the global daemon has loaded that behavior; report those separately.
Production tmux launchers must place sockets in a stable private user directory.
Configure the launcher, preserve existing live endpoints, and verify the actual
new socket path; see [state paths](references/state-paths.md#production-tmux-sockets).

When the user explicitly asks the current peer to become master, that request
provides the role authorization. Replacing a different online holder still
requires the single takeover confirmation in the stable-name contract; ordinary
self-restoration does not. Use the current project's authorized recovery entry.
Do not repeatedly request the same approval, pass an empty
`--approve-grant`, inspect historical journals, or reset the project to obtain
authority. If current context cannot recover, preserve the exact error and fix
that recovery path; do not claim promotion or communication succeeded.

## Task routing and role responsibilities

master 和 peer 的编排功能相同，区别是目标层级：master 管理项目大目标，peer
管理受托子目标，两者都继续拆解、派发、资源管理、验收与交付。peer 不是直接执行
worker；具体一次性编码、执行和观察由子 agent 承担。注册与恢复说明目标层级和
职责，不用任务类型反推协作身份。

Choose the receiver by task kind, not by idle capacity or expected duration. A
fixed command that runs for hours is still bounded work. A goal that needs
planning, delegation, evidence review, unblocking, integration, and resource
closure is a long-horizon goal.

- `master` and ordinary peers primarily own long-horizon goals and orchestrate
  planning, task dispatch, resource management, project acceptance and delivery.
  Prefer assigning code implementation to subagents rather than writing it
  directly. Registration and recovery must explain these roles and recommend
  scheduled patrols for each role's own goals, risks and resources.
- `master` audits, dispatches, verifies, and integrates. It offers each
  long-horizon goal through the public board to a suitable, consenting peer.
  Bounded one-shot execution or observation always uses an independent
  `codex exec --profile gcm` worker. With no eligible peer, retain the long-horizon
  goal and split bounded work. Master retains project coordination, cross-peer
  dependencies, final integration, and the delivery owner's ordinary scope.
- An ordinary peer keeps the code-compatible wire role `worker` and derivation
  `peer`. It is the long-horizon orchestrator for an accepted goal: split and
  plan, run bounded work through independent `codex exec --profile gcm`
  execution or observation workers, verify evidence, resolve blockers,
  integrate assigned work, close owned resources, and report to the sender. It
  does not inherit master authority.
- A managed-subagent record is legacy compatibility state. It preserves
  historical facts for diagnosis or migration and grants no new production
  execution authority.

### Register the long-horizon goal before dispatch

Master and peer must explicitly register their own accepted long-horizon goal
through the supported goal control entry. Master owns the project goal; peer
owns the delegated subgoal. On registration/recovery and each new assignment,
read the existing goal, resume or update it, and create one only when missing.
Record the objective, scope, acceptance evidence, current owner/name and parent
goal when delegated. Report the goal path/identifier and registration receipt.
An ordinary worker task, board assignment, note, timer subscription or verbal
promise does not register a long-horizon goal. Do not report registration as
complete without the goal control receipt.

The AppSDK entry is `appsdk goal subscribe --goal <existing-goal.md> --interval <agreed-period>`;
inspect your own goal with `appsdk goal status --json` and
`appsdk longhorizon show --json`. `appsdk longhorizon` is a read projection,
not a register/subscribe command; `collab longhorizon` does not exist. The
goal entry selects the authenticated caller's project and stable name. Master
and peer each register, read, update, rearm and cancel their own goal. SessionID
is the execution carrier; a worktree or transport change does not transfer
ownership. Master must not proxy-register, hold or cancel a peer's goal. Each
owner's status reconciliation must leave other owners' records unchanged.
A runtime that refuses an ordinary peer's own goal has an implementation defect;
report its exact error, do not promote the peer or route registration through master.

Close owned goal scheduling and patrol subscriptions after completion or when
no new task and no actionable work remain. Retain direct-message reception;
assess a new assignment before registering/rearming its goal schedule.

### Registered patrol duties

Patrol is a safety-net wake, never an execution dependency. After registering or
rearming it, immediately continue all ready work under the standing goal. Do
not end a turn with "wait for the next wake" while an authorized next action,
investigation, dispatch, verification, integration or cleanup remains possible.
A future deadline, missing wake or unavailable patrol service does not block
that work. Wait only for a concrete external dependency or uncovered approval,
and continue independent work while that dependency remains pending.

The accepted peer is the sole delivery owner for its scope. Delegation and
requests for help do not transfer that responsibility. Investigate failures,
verify subworker limitations in the real entry, change the in-scope approach
or executor where authorized, and pursue completion without waiting for master
or another peer to solve it. Do not exceed authority or repeat unchanged failures.

A suspected blocker is a request for investigation, not an approved stop. Ask
master promptly with the task and affected acceptance condition, exact failing
entry/input and original error or evidence path, attempted remedies and their
results, why authorized alternatives cannot proceed, the external dependency,
and the exact decision and resume condition. Separate facts from hypotheses;
a proven root cause is not required to ask for help. Stop only unsafe or truly
dependent actions and continue the rest. Master must independently inspect or
reproduce the relevant evidence and check permissions, alternatives and scope
before accepting the blocker. A peer report alone is insufficient. Record the
decision, verified evidence, affected work and resume condition; return an
unsupported claim for investigation. Approval does not relieve peer ownership
unless master explicitly reassigns the scope. Never close unfinished work merely
to silence a blocker or its reminders.

Registration returns a patrol reminder, and every dispatch repeats it. On the
next authorized activation after registration, promotion, delegation, or
recovery, update the existing scope; context restoration does not fabricate
board tasks, timers, or subscriptions.

This contract is project-agnostic. Role-specific sections elaborate it, but
they cannot replace it with project-specific paths, commands, issue bindings,
or implementation details.

1. A `master` patrols only its own project. The patrol covers open P0/P1 work,
   real peer evidence, owned worktrees and stale resources, canonical issue
   intake, and currently supported monitored events. Use durable board, task,
   delivery, cleanup, and issue evidence; inactivity, heartbeat, and ACK prove
   neither progress nor completion. Close each P0/P1 with applicable evidence
   or park it with owner, cause, and unblock condition. Dispatch ready P0/P1
   work before ending the scheduling turn.
2. Offer delegated long-horizon work through the public board to a consenting
   peer when one is available; keep ownership of your own goal. With no suitable
   peer, retain it and split ready bounded work for subworkers.
   Each accepted peer partitions the goal into non-overlapping bounded scopes
   and orchestrates independent `codex exec --profile gcm` workers. Planner and
   Reviewer remain independent roles. Dispatch bounded one-shot execution or
   observation directly to independent GCM workers regardless of whether other
   peers are busy.
3. A peer owns planning, execution, verification, unblocking, integration, and
   resource closure for its accepted scope. Master verifies returned evidence,
   coordinates cross-peer dependencies, integrates the delivery, and closes
   project responsibilities. Neither role may infer completion from a message,
   receipt, or report.
4. Use Close-Before-Open/SESE at task-domain boundaries: prove the old owned
   worktree is closed before opening its replacement. Dirty, active, shared, or
   unknown resources are PROTECTED_HOLD. Preserve and report them. Do not run
   broad process kills. Do not perform daemon or tmux lifecycle actions,
   including starting, stopping, restarting, or terminating a host pane.
5. Send each new production issue through the project's canonical intake and
   deduplication before repair or dispatch. Record evidence according to the
   owning project contract; project-specific error codes, wire status, and
   response planes stay in the owning project Skill.
6. Continuous patrol uses existing goal or deadline one-shot wakes. Before a
   turn ends while scheduling continues, explicitly re-arm a consumed one-shot
   wake. A read, ACK, status summary, or dispatch receipt is not completion and
   does not renew a wake. Do not invent a periodic polling loop.
7. Every dispatch states the goal, scope, authorized actions, resource owner,
   delivery and test evidence, report channel, and stop conditions. The
   receiver checks the contract against current responsibility, then accepts,
   rejects, or negotiates before work starts.

8. For its own ongoing long-horizon goals, an accepted peer creates and
   maintains finite patrol tasks and subscriptions. On each master assignment,
   first evaluate the goal, dependencies, blockers, priorities and resources;
   then update the existing patrol tasks and subscriptions as part of that
   assessment. This housekeeping does not gate ready authorized work.
   Add only missing scope, retire obsolete owned scope, and do not duplicate
   timers. Report the resulting patrol scope or the concrete reason no
   subscription change is needed.
9. When a task completes, the owning master or peer actively closes that task's
   patrol tasks and timed subscriptions. If there is no new task and no
   actionable work, close the patrol; do not renew, recreate or keep idle
   polling loops. Keep the direct-message receive subscription for new
   assignments. On a new assignment, assess the required scope before starting
   another patrol. Close only owned patrol scope; do not cancel another goal's
   subscriptions.

A legacy managed-subagent record creates no new patrol task, subscription,
dispatch, or execution authority. Its parent or master preserves it only for
diagnosis or migration.

## 稳定名字与旧版 display-label 迁移

项目名字由 daemon 分配和恢复，承载工作归属，不再只是可以重复的显示标签。
Jenny 对应项目 master；普通名字按固定池顺序分配，各项目独立。同名字恢复或换
session 时继承工作并同步未关闭任务，不能恢复 pane ID 默认名或阻止名字工作继承。
内部 worker ID 仍可作兼容索引，但不能代替名字归属。

旧版 `collab worker name` / MCP name 接口曾只修改 label，不能用旧接口的行为模拟
新名字接管。升级以当前 `collab context` 与名字恢复合同为准，读取其权威回执；
旧 runtime 未加载新实现时明确报告版本边界，不把改 label 当作恢复成功。

## Current-version baseline

Use only the current globally installed `collab` and `collab-mcp`. The
`Cargo.toml` version is the semantic source baseline; every official release
compile uses `scripts/build-collab.sh`, which increments the host-global
`~/.collab/build-version` counter under one lock. The installed binary's
`collab --version` reports `0.2.NNNN` as the runtime build version. Direct
release builds fail with an instruction to use the official entry. An upgrade
targets the current reviewed source and does not migrate, replay, or interpret
older local versions.

The canonical candidate delivery sequence is a single source, single
sink DAG. Every applicable node must finish before claiming delivery:

```text
latest_main_candidate
  -> targeted_tests + build/install
  -> user-managed runtime activation when applicable (outside the agent interface)
  -> installed binary + collab context + affected bridge service live checks
  -> black-box live behavior replay + author debug complete
  -> applicable independent architecture review PASS
  -> commit/merge/push and cleanup
```

For ordinary peer bootstrap and identity recovery, the only entry remains
`collab context`. Each AppServer context call carries its current session,
thread, endpoint and namespace from the host or this call's `--provide`.
Desktop CLI uses the current scoped bridge capability described above.
The managed MCP entry can also supply it from executor metadata; invoking that
tool is optional, and agent sessionID remains the identity anchor.
That command is not a daemon lifecycle owner. A Collab runtime
delivery that changes server, notification, identity, route, MCP, CLI, or daemon
behavior must cross the runtime lifecycle boundary in the same delivery unit
after installation, unless the owner explicitly records why no daemon change is
applicable.

The canonical install sequence from the candidate source is:

```sh
scripts/install-global-collab.sh
```

The installer performs one release build and installs those exact candidate
bytes; it does not run a second build with another auto-incremented version.
The canonical pair is `$CARGO_HOME/bin/collab` and
`$CARGO_HOME/bin/collab-mcp` (default `$HOME/.cargo/bin`). The sequence invokes
the exact newly installed binary to refresh the embedded Skill, so an older
PATH entry cannot write a stale Skill. The install does not remove business
source, Git history, `~/.collab/`, project-local `.agent-collab/`, AppSDK
state, run notes, or shared evidence.

Legacy user-local copies are not removed automatically by this sequence.
If an exact old copy must be retired, first prove it is Collab by running its
own `--version` (and for MCP, its `initialize` response), then remove only the
verified pair. A path that cannot prove that identity is a collision: preserve
it and report the exact path. Never delete `~/.local/bin/collab*` or
`~/.local/lib/collab/*` merely because the pathname matches.

Installing a binary does not replace the running user-managed daemon. Agents
never perform its lifecycle actions or request maintenance credentials. After
user-managed activation, verify the available service:

```sh
collab context
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"codex","version":"0.0.0"}}}\n' | collab-mcp
```

Never use a broad process kill or a second daemon. If the live service reports
an older installed version, runtime activation is `INCOMPLETE`, not delivered.

Before changing the installed binary, inspect the current source version,
installed version, canonical paths, and daemon PID/socket:

```sh
cargo metadata --no-deps --format-version 1
collab --version
command -v collab
command -v collab-mcp
collab status --all
```

If the version or command path is stale after installation, fix PATH or refresh
the shell command cache (`rehash` in zsh, `hash -r` in bash), then verify that
`command -v collab` and `command -v collab-mcp` resolve to the exact
`$CARGO_HOME/bin` pair.
Do not hand-copy binaries, leave a second managed entry, or select an older
binary as a fallback.

## One lifecycle loop

Every master goal, bug report/fix, master-to-subagent assignment, and peer task
uses one authoritative `Trigger -> Work -> Gate -> State -> Stop` loop:

1. Discover current durable truth, ownership, dependencies, and evidence.
2. Persist durable dispatch intent before attempting notification.
3. Hand off one scoped assignment with delivery and test conditions.
4. Verify the real result; unknown, timeout, and failure remain explicit.
5. Persist the verified state, evidence, blocker, or failure.
6. Schedule the next eligible work, or stop at the loop's terminal condition.

No ACK, fallback, retry, snapshot, or notification may fabricate success or
replace a missing gate. Bug reports and fixes enter the AppSDK or git-bug
backlog with investigation evidence. P0 is highest priority and blocks the
affected project.

## Recovery, Failure, Reset, Regression

### 1. Recovery

Start with the one-step bootstrap. `collab context` is the single automatic
state entry. The CLI resolves the current cwd project, connects to the existing
user-managed daemon, creates a missing baseline when applicable, observes
runtime facts, and sends a typed identity request to the daemon.
The daemon owns identity selection, creation, recovery, update, credential and
binding persistence, route publication, and default direct-message lease
restoration. Initial registration uses the selected adapter's basic capability
and the existing typed Register commit without another participant challenge.
A current valid pane selects tmux; unselected AppServer facts do not block it.
It returns a registered snapshot or an explicit incomplete/error result.
The active contract is
[`docs/design/collab-identity-minimal-interaction.md`](../../../docs/design/collab-identity-minimal-interaction.md)
with recovery semantics in
[`docs/design/collab-anchor-restore-model.md`](../../../docs/design/collab-anchor-restore-model.md).

```sh
collab context
```

An optional explicit diagnostic may check the restored route through
`collab context --recover-communication` and a challenge/reply. It is not a
recovery prerequisite, and ordinary `collab context` does not wait for it.
When used, the participant reads `COLLAB_REGISTER_CHALLENGE <JSON>` through the
selected adapter and returns it with
`collab context --return-challenge '<JSON>'` from its own environment. The
daemon checks scope, purpose, operation, command, nonce and selected target as
factual transport-safety checks; they do not establish historical identity or
authority. Query remains read-only. A failed diagnostic leaves the restored
identity and current binding intact.

For tmux, cwd selects the project; the stable name owns the work and the agent
sessionID is its current carrier. A new pane or tmux session changes placement,
not ownership. Same-name session changes automatically update unclosed tasks.
With no project master, context restores a peer; an existing master does not
block ordinary peer recovery. A different online name holder requires the
one takeover confirmation; self-restoration does not. Native session/thread
facts locate the selected adaptor and never replace the stable-name contract.

Ordinary persisted restore reuses the identity and restores its current
communication binding, route and default lease without another Register or
mandatory communication challenge. Task content, worktree and old pane
claimants do not gate restoration; historical pane identities and routes are
retired. Actual I/O or persistence errors remain explicit without daemon
lifecycle steps. Acceptance of a restored peer and a restored master requires
each to send, receive and reply through the current route. A durable send or
role alone does not prove message consumption.

For the optional master choice, `--approve-grant` carries `action=restore_master`
and `authorization` containing the user's words. The daemon derives the
current identity, project/session and incumbent and uses the authority owner to
replace it directly. Do not require target snapshots, old credentials, digests,
grant generations or a second identity approval. Omitted approval leaves peer
restoration successful and the incumbent unchanged.

Tmux identity uses current project and socket/server/session facts. Pane, pane
PID, native thread, activity and capture do not select the identity. Endpoint
status and message delivery remain separate from identity restoration.

The CLI reads `DSH_SESSION_ID`, `CODEX_SESSION_ID`, `CODEX_THREAD_ID` and
current pane facts. A unique saved DSH identity restores from its current
session in the same scope without Register or a communication challenge.
Gateway unreachability does not revoke that identity or its work.

For a new DSH identity, run ordinary `collab context`. With `DSH_SESSION_ID`,
the CLI reads the gateway-published control capability and resolves that exact
session in the current project. The gateway supplies current runtime facts;
the agent does not inspect logs, plists or runtime state files. Existing
registered sessions recover through their committed route and do not depend
on this bootstrap declaration. A missing declaration is a gateway integration
gap; an unreachable or rejected declared endpoint stays an explicit error.

For an explicit caller-provided DSH capability, report actual facts through
`collab context --provide '{"dsh":{"endpoint":"unix:///actual/control.sock","runtime_id":"actual-runtime","agent_id":"actual-agent","session_id":"actual-session","cwd":"/actual/project"}}'`.
MCP `collab_context` accepts the same object in `provide`. Obtain these facts
from the current DSH gateway API; do not guess them or reuse stale logs.
The daemon verifies the report and uses the existing DSH registration owner.
Do not supply Codex thread or namespace fields for this path. An explicit
API conflict fails without replacing the saved identity, role or work.

When the snapshot contains `requires_identity_update`, read
`required_fields`, `reason`, and `exact_error`. Supply requested current facts
from their real source in this invocation. The AppServer form is:

```sh
collab context --provide '{"session_id":"...","thread_id":"...","endpoint":"...","namespace":"..."}'
```

Each AppServer context invocation carries the current session, thread, endpoint
and namespace. The daemon never fills these facts from historical registration.
AppServer CLI facts come from the current host or this invocation's `--provide`;
AppServer MCP callers submit them on each call because startup facts may be stale.
Desktop uses the separate bridge capability through CLI or the optional MCP
entry, not this AppServer supplement.
Missing facts preserve durable identity and work but do not complete restoration.
`--query` only reads retained progress and needs no current endpoint.
See [current-fact recovery](references/stable-name-recovery.md) for the contract.

The AppServer supplement accepts only the scalar facts `session_id`, `thread_id`,
`endpoint`, and `namespace`. It rejects any other key. An agent never guesses,
selects, or hunts a worker, and never supplies `worker_id`, approval, token,
generation, binding, or project scope. Invalid or unsupported input fails
explicitly. The returned snapshot is the complete state read: role, operations,
peers, master, scheduling, tasks, binding, and filtered environment. `binding`
comes from the daemon ledger when the caller has a unique binding.

If `collab context` fails explicitly, preserve the original error and affected
scope. Do not retry automatically, edit route/identity files, copy tokens,
start another daemon, or invent a fallback. Daemon lifecycle is outside the
agent interface. Ordinary identity recovery uses `collab context` only.

Master authority is separate from identity. The authority owner is the current
typed grant in the exact project and app scope, with only two states: empty (no
current grant) and assigned (one current holder). When the user explicitly
approves this session for this exact project, run one command with their words
as the approval:

```sh
collab master promote --approval '<why the user approved this>'
```

An approved promote atomically replaces any recorded holder and is not gated on
the incumbent's or the candidate's transport liveness. A tmux pane and an
AppServer oracle are addresses, not credentials: neither being reachable is
allowed to strand authority. Explicit approval is the whole authority for the
transition. To remove the scoped authority instead, an authenticated peer with
explicit approval runs `collab master clear --approval '<why the user approved
this>'`; clear removes only the scoped grant and never deletes tasks, messages,
peers, or bindings. Context never auto-promotes and never infers authority.

### 2. Failure

Treat each claim separately:

```text
durable=true -> mailbox journal accepted
notification=accepted -> selected transport accepted the operation; it is not consumption
recv receipt -> peer consumed the message
task close receipt -> lifecycle ended
```

For tmux, `send --delivery queued` retains the durable message until the existing
configured event window. It returns `notification=queued`, `consumed=false` and
records no wake attempt while deferred. The existing timer drains it through a
daemon wake; only actual adapter success records submission. Accepted or unknown
native outcomes are not blindly retried. Pane output never supplies a busy/idle
decision or a recv receipt.

An error, timeout, `subscribed-not-sent`, `thread-lost`, `identity-mismatch`,
`unknown`, or absent Agent is not success. Preserve the exact error and
durable IDs; do not retry automatically, ACK for another identity, or mark a
task delivered/closed without its required evidence. Apply Registered patrol
duties for a prompt evidence-based help request and independent master
verification. The peer retains delivery ownership unless explicitly reassigned.

Transport observations (`unknown`, `cold`, `missing`) are separate from
authority. They never create, replace, or clear the current master grant. Do
not promote, clear, or regrant the master merely because a transport probe is
unknown; the holder keeps control, and the communication error stays explicit
and independent.

### 3. Reset

The current project master may reset the project's Collab state online:

```sh
collab reset --project
```

Run it from the current project. The daemon authenticates the current scoped
master, clears project messages, tasks, bindings, routes, subscriptions,
operations and assets, and deletes project history. It preserves other projects
and the global daemon. Historical pane owners, open work and resident host
storage do not block this authorized project reset. The next `collab context`
creates a peer with no inherited master grant. Real deletion or durability
errors return incomplete and retain the scoped reset intent for recovery.
No legacy discard mode or separate approval argument exists for this project
reset. Host maintenance is outside the agent interface.

`.appsdk/` and `.appsdk-control/` are AppSDK-owned and are not removed by
`collab reset`. The host-wide runtime truth remains `~/.collab/`
(`server.sock`, `events.jsonl`, `log.txt`, and route state); project-local
`.agent-collab/` is reducer input and local durable data, not the global truth.
For a new project or an explicitly authorized clean epoch, remove old
project-local governance only through the owner's canonical reset/migration
command. Never manually remove `.agent-collab/server/journal.jsonl`, mailbox
files, identity tokens, task records, or bindings to make status look clean.
Never start a second daemon or use broad process kills. `collab ack` remains a
compatibility operation; it is not a substitute for task close or identity
recovery.

An AppSDK `reset-governance` or `appsdk init --fresh --discard-legacy` is not
Collab reset/migration and must not remove `.agent-collab/`. AppSDK's reset owner
explicitly treats `.agent-collab/` as a reserved root. If the project also
needs to move or retire Collab state, follow
[Migration and User-Managed Daemon Boundary](references/migration-daemon.md); the two
operations have separate transactions and separate completion evidence.

### 4. Regression recognition

After a fix, verify the same user path again and classify the first divergence:

- `send` durable but no transport acceptance: inspect the selected transport,
  subscription, ownership, Agent state, and daemon log.
- tmux input submission appears but no worker result: inspect the durable
  mailbox receipt and peer state; do not call submission a reply.
- `recv` returns messages: the read is consumed atomically; no follow-up ACK is
  required. `msg`, `inbox`, and `context` remain read-only.
- task remains open: inspect owner identity, master responsibility, cleanup
  receipt, and notification supersession.

Record the tested source commit, installed version, daemon PID/socket, exact
commands, and live replay result in the bug system. A test pass without
latest-main merge, installed-binary verification, or applicable live replay
does not close the bug.

## Registered multi-worker collaboration

Only successfully registered participants use this collaboration protocol.
Multiple workers alone do not enable Collab. For an existing registered peer,
run `collab context` once in its own live environment to recover current state
and the default finite direct-message subscription. Initial registration is
performed only when explicitly requested by the user; if it cannot complete,
preserve the error and continue independent work without this protocol.
Registration returns `role_brief`. Read it as the active operating contract.
It is the registration-time projection; `collab context` returns the current
brief, and promotion or delegation returns the replacement brief. Do not
maintain a separate role prompt:

- `master`: orchestrate the project goal, offer delegated long-horizon goals
  through the public board, own blockers and drive verify/merge/cleanup/close.
  Send one-shot execution or observation directly to independent GCM workers.
  Peer availability does not gate this routing.
- `worker` with derivation `peer`: orchestrate the accepted long-horizon goal,
  run bounded work through independent `codex exec --profile gcm` workers,
  verify evidence, resolve blockers, integrate assigned work, and report to the
  sender. Evaluate master invitations against current ownership and capacity.
  Do not inherit master authority.
- `managed-subagent`: legacy compatibility only. Preserve historical facts;
  accept no new work and create no schedule, dispatch, registration, or
  expanded authority.

The default identity is peer. Master authority is explicit and
user-authorized: an approved `master promote` replaces the recorded holder,
and delegation is accepted only from the current master grant holder and
records that handoff. Registration, a process, inferred `/goal`, or
`role_brief` never silently creates master authority.

On trouble, apply Registered patrol duties: investigate and promptly report
available failure evidence, attempts and the decision needed to the current
master. Mark an unknown cause as unknown; help does not require a proven fix.
A role change via `master promote` or `master delegate` returns the new master brief;
the old worker brief no longer governs that peer.
Once task scope and the independent worktree are known, automatically follow
[task/worktree registration](references/task-worktree-lifecycle.md): bind the
task, feature/resource and owned file scope before concurrent product edits.
Use [resource coordination](references/resource-waits.md) for overlaps; never
share a worktree or overwrite another peer's files.

Registration, necessary coordination and subscriptions require no repeated
user confirmation within an authorized multi-worker task. Send only messages
authorized by the user's collaboration request; no unrelated external notices.
Do not require a serial merge queue for communication or read-only work.

If initialization fails, report collaboration unavailable and preserve its
error. Shared writes and dependent coordination wait for reliable ownership;
independent isolated work and AppSDK quality checks can continue. No invented
peer, local substitute claim or fake successful registration.

## Send an ordinary message

Run exactly one command:

```sh
collab sendmessage --to <peer> --subject <short-topic> "<original message>"
```

`--to`, a non-empty short `--subject`, and the original body are required.

Do not first run `notify methods`, `notify subscribe --help`, or a separate
identity probe, or choose `mailbox-only`. There is no separate mailbox-only
send mode.
`sendmessage` always commits the full subject/body to the durable mailbox.
With a matching live subscription, the first pending message opens a fixed
120-second window by default. `~/.appsdk/config.toml` can select immediate or
batched delivery globally or per project; `appsdk config` shows effective
policy. All eligible unsent messages for that recipient are combined
into the selected transport (up to 3 previews per notification, with overflow
retained in the inbox). On a Desktop bridge binding, the daemon calls the
bridge's native `send_message_to_thread`; the sender still uses the CLI.
On an AppServer binding, the daemon uses
`turn/start`, `turn/steer`, or `thread/queue/add`; on a tmux-only binding it
submits a bounded wake to that pane. Neither transport acceptance proves that
the peer consumed the durable message, and neither binding falls back to the
other transport after selection.
Explicit `collab sendmessage` is
immediate and follows the explicit-message adapter gate, including while the
recipient is working.
If delivered-but-unconsumed notifications reach the throttle threshold (default
3), further push knocks pause until `collab recv` consumes them, preventing
terminal pollution and storms. Each batch has one attempt; the default window
is 120 seconds. Policy changes take effect through user-managed runtime
activation, not task reset.

Explicit `collab sendmessage` is immediate. Idle, progress, delivery, bug, and
worker-idle notices are auto-merged by the daemon in the 120-second batch
window; they are not repeated as heartbeat storms.

Do not retry a failed send automatically. Return its exact error and durable
status. Never call a transport command directly; the server owns transport
selection and sends only through the selected adapter.

## Public task board and read-only dashboard

未关闭任务保留；负责人身份的当前绑定失效时，board 负责人显示为空，由 master 重新安排。读取不恢复旧身份、不自动关闭任务，也不修改历史 owner、资源或证据。已关闭任务保留历史负责人。board 是独立业务读取，不参与身份恢复判定。

The task board is the shared work surface for registered masters and peers.
Each active role declares its current work and may queue later work here.
Legacy managed records remain visible only through their existing
compatibility scope; they receive no new assignment. The public view carries
task identity, lifecycle status, owner, and the typed delivery/test conditions.
It never exposes worker tokens, runtime endpoints, role briefs, managed-child
identity, private assignments, or raw journal events. A published task does not
start execution; an invitation does not transfer ownership.

Read the board without consuming messages or changing lifecycle:

```sh
collab board show
```

Any registered execution role publishes a pending task with its contract.
Publishing queues work; it does not start it. Publish several task IDs to queue
later work without consuming execution capacity:

```sh
collab board publish <task-id> --title <t> --description <d> \
  --delivery-condition <d> --test-condition <t> [--priority p0|p1|p2|p3|p4]
collab board claim <task-id> --expected-revision <observed>
```

Claim starts one pending task in the caller's own identity. A peer may claim
its own publication or another authorized task. Read the current revision from
`board show`. Claim atomically checks that the task is still pending and that
the caller can start execution; it records the actual executor as owner. A
stale or competing claim fails without taking the task. Do not update a
pending task to `working` to bypass claim. Never claim outside the user's
authorized goal or take another owner's working or blocked task.

For a long-horizon goal, master may instead offer a public task to one ordinary
peer:

```sh
collab board invite <task-id> --to <peer> --expected-revision <observed>
```

Invite and respond are revision-checked. `invite` fails with
`BOARD_STALE_REVISION` when the observed revision is old, and with
`BOARD_PEER_BUSY` when the target already owns an unfinished task or another
invitation. Take the revision from `collab board show`, never from a message
body.

The invited peer accepts or declines in its own identity. Accept rechecks the
peer's other responsibilities and its live binding before ownership moves:

```sh
collab board respond <task-id> --accept --expected-revision <observed>
collab board respond <task-id> --decline --expected-revision <observed> --reason "<text>"
```

Decline requires a nonempty reason and leaves publisher ownership unchanged. The
owner updates progress on its own task with a current revision:

```sh
collab board update <task-id> --expected-revision <observed> --status <s> [--next <step>]
```

The master withdraws an unaccepted invitation with its observed revision and a
reason; withdrawal never reclaims peer-owned execution:

```sh
collab board withdraw <task-id> --expected-revision <observed> --reason "<text>"
```

Public work uses claim or the board invitation above. `collab subagent
dispatch` is a legacy private managed path and grants no new production work.
If a peer declines a long-horizon invitation, retain that goal and arrange
another consenting peer or decompose it under the current orchestrator.
Dispatch its bounded execution or observation directly to independent
`codex exec --profile gcm` workers. A refusal does not authorize the legacy
private managed path or transfer orchestration responsibility to a subworker.

`collab dashboard` opens a read-only loopback Web observer for the same public
board. It requires a registered identity, does not bootstrap or restart the
daemon, and never consumes messages:

```sh
collab dashboard [--port <loopback-port>]
```

It prints one JSON line and then serves until stopped:

```json
{"url":"http://127.0.0.1:<port>/#<capability>","read_only":true,"refresh_seconds":2}
```

Open that printed URL in a browser on the same host. The capability is a fresh
per-run read credential carried only in the URL fragment: the page keeps it in
memory, sends it as a bearer header, and never puts it in a query string or a
log. It grants no actor identity and no write authority. The page re-reads the
board projection every 2 seconds; it is a polling view, not an event stream.

Any registered identity on that host may start one, so a master or an ordinary
peer can expose the project board for the human controller. It is an observer,
not a control surface: it cannot publish, invite, respond, update, withdraw or
close anything.

The observer rejects a request whose `Host` is not its own bound authority with
`DASHBOARD_HOST_REJECTED` (403) and a request without the exact capability with
`DASHBOARD_CAPABILITY_REQUIRED` (401). While the daemon is unreachable the
board endpoint answers 503 with the exact daemon error instead of an empty
board. Static assets carry no state and no token.

Legacy managed identities and private child tasks remain compatibility facts.
They do not receive new public assignments. Preserve their historical privacy
and visibility semantics; do not create or bind another child from current
work. New bounded execution or observation uses an independent GCM worker.

An owner edits the descriptive part of its own independent task without
changing lifecycle. Every flag is required and the revision is checked:

```sh
collab board describe <task-id> --expected-revision <observed> --title <t> \
  --description <d> --delivery-condition <d> --test-condition <t>
```

`collab-mcp` exposes the same operations as `collab_board_show`,
`collab_board_publish`, `collab_board_claim`, `collab_board_invite`, `collab_board_respond`,
`collab_board_withdraw`, `collab_board_update`, `collab_board_describe`, and
`collab_task_decline`. Each takes `expected_revision` as an integer of at least
1; `collab_task_decline` takes `legacy_assignment: true` only for an unstarted
legacy assignment.

When any board command fails, the CLI preserves the server's full typed
response, including durable IDs and `repair_required`. Read the `collab
response:` line before retrying: a durable outcome is not a retryable failure.

### Continue the queue and recheck blockers

Use this sequence for master and peer work. Master also checks the public queue
and its coordination tasks. Legacy managed records are inspection or migration
inputs only.

1. Before peer/master board work starts, publish its goal, delivery condition,
   and test condition, then claim it or accept its invitation. For bounded
   execution or observation, give an independent `codex exec --profile gcm`
   worker the same goal, scope, delivery/test evidence, and stop conditions.
   Reuse an existing assigned or registered task; describe it if conditions
   are missing. Do not register a duplicate task for the same work.
   Bind a claimed task's execution worktree through `collab task relocate
   <task-id> --worktree <owned-worktree> [--branch <branch>]`; queue entries
   do not reserve another task's resources.
2. On completion, finish the applicable delivery, review, integration, cleanup,
   and close steps. A delivery report alone does not end a cleanup obligation.
   Read `collab board show` and check for unfinished work before stopping.
3. On a blocker, keep ownership and record the cause, release condition, and
   next action with `board update --status blocked --next "<facts>"`. Report it
   to master. For an actual resource conflict use
   `collab task wait <task-id> --for <holder-task-id>`; it is not a
   general dependency queue.
4. Check owned blocked/waiting tasks against current evidence. If a blocker
   has cleared, recheck resources and execution capacity, then resume through
   `board update --status working` using the current revision. A release notice
   requires a recheck; it does not prove the task can resume. Keep it blocked
   while another task occupies the execution slot.
5. If current work cannot proceed, select another authorized, nonconflicting
   pending task and claim it. Prefer priority p0 through p4, then task ID for
   equal priority. Pending queue entries do not occupy execution capacity.
   Blocked/waiting tasks keep their resources and unfinished responsibility;
   starting another task does not release or cancel them.
6. Before entering idle, check remaining work, pending close/cleanup duties,
   each blocker, owned worktrees and stale resources, open P0/P1 work, and any
   new production issue requiring canonical intake. If any action is possible,
   continue it. Otherwise record what is blocked and the event or responsible
   actor needed to resume. Preserve dirty, active, shared, or unknown resources
   as PROTECTED_HOLD. If continued scheduling is required, explicitly re-arm a
   consumed one-shot goal or deadline wake before this cycle ends. Repeat this
   check on the next message or supported notification. Consume applicable
   messages with `collab recv` on non-DSH transports. DSH receives full messages
   natively through its gateway and acts directly on them.

These are agent actions within an activation. The board does not automatically
claim, execute, or resume work. Idle peers and children have no new periodic
wake guarantee. Do not invent a polling interval, cron job, or retry loop.
Existing subscriptions, master wake, and one worker-idle fact remain unchanged.

## Common command card

Legacy managed-subagent records grant no new production execution authority.
Registered peer status and thread state use the owning AppServer RPC. Use peers
for long-horizon goals. Use independent `codex exec --profile gcm` workers for
bounded execution or observation. Legacy records may be inspected only for
diagnosis or migration.
For registered participants, `collab-mcp` and the `collab` CLI expose the same
collaboration protocol. Listed `collab_*` tools alone do not prove registration.
If MCP is missing, unsupported, aborted, or unknown, run the same
actions with the CLI in the inherited project cwd:
`collab context`, `collab recv`, `collab ack <id>` / `collab ack --all`,
`collab msg <id>`,
`collab inbox`, `collab worker status [id]`,
`collab sendmessage --to <parent> --subject <topic> "<body>"`.
DSH uses native incoming messages instead of the listed `recv` and `ack` actions.
The CLI is a complete protocol path. Missing MCP is not a blocker and
does not justify skipping receive or waiting. `collab context` is the only
agent bootstrap entry; do not run a separate identity, route or archive probe.
A conflicting claim on your tmux pane (a stale claim, or one from another
worker or project) is resolved by the daemon inside that same one call, so you
never adjudicate a pane conflict and never need a second command for it.
`collab init` is not the default agent entry, but it drives the same daemon
identity gate and remains the entry for existing AppSDK init consumers.
Ordinary peer assignment never uses `collab subagent dispatch`. Publish the
public board task, then claim it or invite the selected peer with the observed
revision. Bounded execution or observation goes to an independent
`codex exec --profile gcm` worker. `collab subagent dispatch`, managed-child
`send`, and managed `working` are legacy compatibility only and do not become
a fallback when board admission fails.

## Assignment template

Copy and fill this template for a normal scoped assignment:

```text
Goal:
  Make <feature> work in the project.

Scope:
  Allowed: <paths/modules you may touch>
  Forbidden: <shared/control paths; daemon lifecycle; ~/.collab and
  .agent-collab state; routes, journal, mailbox, tokens>

Delivery iff:
  Complete only when <observable behavior> passes <specific checks>.
  Non-goals: <things not required; unrelated refactors>

Tests:
  Commands: <exact commands>
  Expected: <exact pass output/exit code>
  Evidence path: <file/JSON/log to attach in delivery>

Deliverables:
  <commit SHA>, <diff files>, <test output>, <real entry replay>

Forbidden:
  Do not merge/push, restart the shared daemon, install a global binary,
  edit control state, or touch files outside Scope.

Flow:
  Resolve root with collab context.
  Create/verify the assigned worktree <--worktree-path> on branch
  <--branch> at <--base-commit>.
  Implement scoped change, run tests, commit.
  Deliver evidence to master; use Registered patrol duties for suspected
  blockers, timely help requests and master verification.
```

Board assignment example:

```sh
collab board publish fix-notify-lost-recv \
  --title "recover lost recv delivery" \
  --description "Goal, scope, forbidden paths, flow, and deliverable" \
  --delivery-condition "Observable behavior and required evidence" \
  --test-condition "Exact command, expected result, and evidence path" \
  --priority p1
collab board invite fix-notify-lost-recv --to <peer> \
  --expected-revision <observed>
```

On non-DSH transports, consume notifications promptly with `collab recv`. A successful receive delivers
and acknowledges the batch atomically. After 3 delivered-but-unconsumed
notifications, push knocks pause automatically to prevent notification storms
and prompt pollution; `collab inbox` is read-only and does not resume delivery.
Use explicit `collab ack` only for legacy clients or recovery of an already
delivered message on non-DSH transports. DSH's gateway performs consumption and
acknowledgement; process the native full body directly. Inspect peer/worker health, identity validity, and throttle
status via the `collab context` snapshot. The human diagnostic
`collab worker status [id]` and `collab who` print the same fields when an
operator audits them; neither is part of the default agent flow.

Worker wake model: only master has long-horizon wake; workers are not
long-horizon wake targets and receive no automatic periodic task-continuation wake.
An explicit outstanding reply marker permits one reminder per observed idle episode;
it only asks the recipient to read the original reply requirement. A worker
acts on an explicit dispatch, a bounded direct-message lease, or its own open
task state;
it does not need periodic activation to make progress. Unknown/absent produces
no transport input. On each `working` -> `idle` transition, a worker sends one
idempotent worker-idle fact to the current master. Before stopping, it follows
the board queue and blocker checks above; it continues any eligible work.
If nothing can proceed, it stops and does not keep knocking. Idle, progress,
delivery, bug, and worker-idle notices are
auto-merged; explicit `collab sendmessage` remains immediate. Master idle
reminders are level-triggered: within the same master idle episode, each
reminder attempt consumes the shared episode-local budget, up to three
attempts. An observed change to `working` ends that episode; after three
consecutive attempts without an observed `working` change, the episode stops.
Worker-idle facts do not count toward this budget, and there is no
scheduling-turn counter or automatic rearm.

Legacy managed records do not receive child-targeted liveness work or new
assignments. Their historical `subagent-status` facts remain diagnostic input.
Codex thread history is read only from the owning AppServer when supported and
is never fabricated. A tmux pane locates delivery and observation only; it is
not an identity anchor, task owner, or snapshot receipt.

## Cross-project master communication

### 权限、目标解析与下一步动作

这是一条已存在的项目 master 到项目 master 通道。工作角色（designer、reviewer）
不授予此权限。普通 peer 把跨项目请求交给本项目当前 master；不得换 cwd 冒充目标
项目的 agent，不得直接写 mailbox、猜 session ID，或通过另一个宿主通道绕过拒绝。
全局 CEO 是另一个跨项目协调层；在 CEO 的控制入口实现前，不把 CEO 称呼当作授权。

发送者在自己的项目执行 `collab context`，使用返回的当前身份和 master 授权。
用户提供目标项目绝对路径后，可以只读查询目标当前 master；这个查询不注册身份：

```sh
(cd '/目标项目绝对路径' && collab master status)
```

读取回执的 `master.worker_id` 作为 `--to`，不要按 Jenny 等显示名搜索历史 session。
查询失败不代表没有 master。已有 master 但未知、不可达或投递失败时，保留原错，
不创建替代 master。只有真源明确无 master，才向用户说明项目路径、profile 和启动方式，
经确认后创建新的具名 tmux Codex session 并授权 master。默认 profile 为 `gcm`，
使用用户已确认的 YOLO 模式；不得关闭现有 tmux server 或 pane。

从发送者自己的项目目录执行下列命令；`--project` 只指定接收项目，不改变发送者身份：

Collab communication across projects is master-only and explicit. Only the
current master grant holder may send to the current master grant holder of
another initialized project; non-master peers and managed subagents are
rejected before any message is persisted:

```sh
collab master send --project /abs/path/to/target --to <target-master> \
  --subject <short-topic> "<original message>"
```

`--project` must be the exact target project root with `.agent-collab`, and
`--to` must be that project's current master grant holder, whose target binding
and address must be available. The target daemon also verifies the sender's
current typed grant (holder, scope, approval) before accepting the message.

接收 master 读取完整原消息并执行已授权动作。非 DSH 宿主用 `collab recv`；
DSH 直接处理 gateway 已投递的正文，不再 Poll/ACK。回信仍在自己的项目执行
`collab master send`，将原消息 `from` 中的源项目用作 `--project`、源 worker ID
用作 `--to`；当前 CLI 的这条命令尚不支持 `--in-reply-to`，在正文中写明原消息 ID。
这只提供可读关联，不宣称已清除 daemon 回复标记。持久入队、宿主投递、接收消费和
实际回信分别核实，发送成功不能证明对方已经执行。

可直接给 agent 的下一步提示词：

> 先在自己的项目执行 collab context，核对你是当前 master。对用户给定的目标项目
> 用 `(cd '<目标项目>' && collab master status)` 只读获取 master.worker_id。
> 回到自己的项目，用 `collab master send --project '<目标项目>' --to '<worker_id>'
> --subject '<主题>' '<具体action、范围、验收和回信要求>'` 发送。不要搜索历史 Jenny
> session，不要用 sendmessage 跨项目投递。保存 msg_id，分别报告已入队、已消费和
> 已收到实际回信的证据；只获得哪一步就报告哪一步。拒绝或未知状态保留原错。

Task liveness is an obligation, not an ACK ceremony. A worker owns its assigned
tasks and drives them to verified cleanup/close during its working cycle. This
is a task-bound inspect obligation, not a transport activation schedule: it
does not wake idle workers and does not generate worker transport input. If an actionable
task is open, continue it; if an ordinary peer is blocked, first ask the current
master for help with the available evidence and decision needed. A proven root
cause or fix is not a prerequisite for that request. Do not leave a task
at `assigned`, `working`, `blocked`, `waiting`, `delivered`, or
`cleanup_pending` merely because the last direct message was acknowledged.
After delivery or merge, perform the real cleanup and close the task; a
reminder does not create a second task or a duplicate dispatch. After closing
or recording a blocker, inspect the board and continue the next eligible task.
Enter idle only after the remaining-work and blocker checks above.

Escalation routing is explicit:

- An ordinary worker reports blockers to the current Collab master immediately.
  Do not wait for the next liveness cycle or for master to invent the fix.
  Supply available failure evidence, attempted remedies and the decision
  needed; distinguish unknown cause from verified facts. Do not delay an
  assistance request until the root cause or solution is proven. Do not
  dump "I'm blocked" and idle. If the assignment's delivery or test conditions
  are ambiguous, do not guess; propose the missing conditions and send them to
  master. Independent peers may temporarily decline a master collaboration
  invite to protect their own current task. If no current
  master exists, report to the collaborator that initiated the task. Include
  the task ID, failure evidence, attempted actions/results, and requested
  decision. Include a cause or proposed solution when known; otherwise state
  unknown and request help immediately.
- Master authority is the current typed grant in the exact project and app
  scope. Only two authority states exist: empty (no current grant) and
  assigned (one current holder). When the user explicitly approves this peer
  for this exact project, record it with `collab master promote --approval
  "<user text>"`; the approved promote atomically replaces any recorded holder
  and is independent of the incumbent's or the candidate's liveness. Only the
  current master grant holder may `collab master delegate <peer>`. An
  authenticated peer with explicit approval may `collab master clear
  --approval "<user text>"` to remove the scoped grant; the caller does not
  need to be the master first. An internal init adapter result alone never
  proves master ownership, and AppSDK init is not an implicit authority reset.
  Read the current holder, its scope, and its approval from the `collab
  context` snapshot and `collab master status`; agent liveness is a separate
  transport fact and does not change authority. A missing worktree-local
  `.agent-collab/`, a failed `collab context`, a token mismatch, or a missing
  `who.master` field never authorizes a mutation; promote, clear, and delegate
  still require an authenticated current binding. Codex root is not Collab
  master. Operators may use `collab master status` read-only for audit.
- If an evidenced dependency cannot be resolved locally, report the evidence
  and requested decision to the current master immediately instead of
  silently waiting. Keep the durable wait/task state, continue any
  independent work, and re-escalate on the next direct master communication or
  when the situation changes; do not wait for a periodic worker wake.

## Master owns the outcome, not the excuse

Collab master is accountable for the final result of every assigned task
in the project. Once master accepts a dispatch, the master -- not the
worker -- is the escalation target, and the master cannot hide behind the
worker's blocker. Concretely:

- The current master may use `collab task close <id> --force --reason "<text>"`
  for an authorized cancellation or verified lifecycle cleanup. Record the
  actual outcome and remaining work in the cleanup receipt. A blocker alone
  never requires force-close and cannot justify reporting unfinished work as
  delivered.
- When a peer reports a blocker, apply the evidence and approval contract in
  Registered patrol duties. Master owns cross-peer decisions and assistance;
  the peer retains delivery ownership unless explicitly reassigned. Do not
  accept a report as proof or force-close unfinished work to stop reminders.
- When no master is assigned, the task owner may force-close its own task with
  `collab task close <id> --force --reason "<text>"`. If the owner's registered
  transport identity is missing and no master is assigned, a registered peer
  may close that orphaned task with the same command. An unreachable or unknown
  master remains assigned and does not permit these exceptions. These are the
  only allowed force-close exceptions;
  the reason and cleanup receipt are mandatory so the daemon can show who
  closed what and why.
- Master retains project accountability: verify, coordinate, unblock or
  explicitly reassign. Keep genuine external waits scoped and evidenced,
  continue independent project work, and preserve unfinished-work truth.


## Master splits and assigns

The human remains the only final authority for goals, money, irreversible
risk, and version promotion. Collab master is the user-approved project
dispatcher, not the human 主脑 and not Codex root. Master compiles
the goal into a task graph, then assigns; it does not take another peer's
task or worktree.

**Master is an architect and dispatcher, never an everyday code-author.**
Master authoring business diffs is an anti-pattern and a failure of division
of labor. Master's scarce capacity belongs to task graph compilation, strict
dependency boundaries, unblocking workers, and driving overall throughput.
Master operates under two prime directives:
1. **Exemplary Task Decomposition**: Slice goals along clear dependencies and
   non-overlapping file ownership. Every dispatched task must be closed-loop
   by design: define explicit done-iff (DoD), artifacts, forbidden edits, exact
   test commands, expected results, and evidence location. Every assignment must
   anticipate failure and define an exception resolution path: **fallbacks,
   silent downgrades, or masking errors are strictly prohibited**. A blocked
   peer supplies failure evidence, attempted remedies and the decision needed
   for master verification; an unknown root cause does not delay requesting
   help. Master must actively close the lifecycle rather than patch
   output symptoms.
2. **Peer-first Capacity**: Master must keep eligible capacity saturated
   without serial bottlenecks. Offer long-horizon goals through the public
   board to suitable, consenting peers. Each peer orchestrates independent
   `codex exec --profile gcm` workers for bounded execution or observation.
   Master and peer send one-shot execution or observation directly to GCM
   workers, regardless of peer availability. Before ending each scheduling turn, inspect ready work, peer
   capacity, protected holds, P0/P1 work, and the active issue backlog.
   Delivery, merge, review, task close, and cleanup are lifecycle steps, never
   reasons to leave eligible work undispatched.

**Sovereignty and Backlog Priority**:
- **No autonomous technical debt refactoring**: When assigned tasks complete
  and the fleet becomes idle, Master is strictly forbidden from autonomously
  launching long-range technical debt refactors, speculative architectural
  rewrites, or unapproved work. Master must formulate a structured proposal
  for the human user and pause. The human is the ultimate decision-maker;
  unbounded autonomous runs risk destabilizing user intent.
- **Autonomous Bug Tracking Backlog Resolution**: If pre-existing issues or
  requirements are logged in the bug system (`appsdk bug list --status open`),
  these represent authorized project work. Master autonomously pulls and
  dispatches open bugs in strict priority order (P0 > P1 > P2) to keep
  worker capacity saturated before suggesting closure.

**Worker->Master idle fact and master long-horizon wake**:
When a worker transitions from `working` to `idle`, it emits one idempotent
worker-idle fact to the current master. The master has long-horizon wake; the
worker does not. On an idle fact, master must:
1. Check the active task graph for unblocked downstream tasks and offer them
   peer-first through the public board;
2. If the main graph is clear, pull the highest-priority open issue from the
   bug backlog (`appsdk bug list --status open`), with P0 first; P0 blocks the
   affected project;
3. Inspect owned worktrees and stale resources, preserve PROTECTED_HOLDs, and
   re-arm a consumed one-shot wake if continued scheduling is required;
4. If all tasks and issues are closed, report evidence-backed completion and
   propose next steps to the user.

Do not stop after consuming a report or closing a task while authorized ready
work has no correct receiver. Offer long-horizon goals to peers through the
board. Use independent `codex exec --profile gcm` workers for bounded execution
or observation. Dispatch the next ready non-overlapping P0/P1 work before the
scheduling turn ends. Master and peer retain their goal's orchestration and
delivery responsibility when dispatching bounded GCM work.

Master idle reminders are level-triggered. Within the same master idle episode,
each reminder attempt consumes the shared episode-local budget, up to three
attempts. An observed change to `working` ends that episode; after three
consecutive attempts without an observed `working` change, the episode stops.
Worker-idle facts do not count toward this budget, and there is no
scheduling-turn counter.
Master must not expect workers to be woken periodically. A read, ACK, status
summary, or dispatch receipt does not complete a patrol or renew a one-shot
wake.

**Unacknowledged Workers & Pane Diagnostic Closure**:
If a worker fails to acknowledge notifications or remains unresponsive across
repeated dispatches, never blindly loop sends or expect the model to self-correct.
Execute diagnostic closure immediately:
```sh
collab worker status <id>
```
Use durable Collab state and the selected binding's status source to choose the
next action. For an AppServer binding, query native thread state. A tmux pane is
address proof only and never proves the agent is alive or `Present`; its agent
liveness stays unknown. Transport status cannot establish message consumption
without the receive receipt. A tmux screen cannot establish task completion,
peer consumption, or Codex turn state. Do not infer those facts or restart a
shared runtime from a pane probe.

Master keeps architecture, dispatch, integration, critical repair, and final
acceptance. Use a board invitation for a long-horizon peer goal. Use an
independent `codex exec --profile gcm` worker for bounded execution or
observation. Legacy managed-subagent records grant no new execution authority
and fail closed for unsupported thread creation. Give each peer and GCM worker
its own worktree and file scope. Independent peers may decline an invite to
protect their current task. Wait for evidence summaries, verify them, then
integrate. Chat tone is not completion.

Delivery and review are not lifecycle endpoints. `task deliver --no-candidate`
explicitly classifies read-only work with no code candidate. When review
accepts it, the typed disposition is `no_merge_required`; no pending merge or
merge wake is created, and the owner can close normally after owned cleanup.
`task integrated` is not applicable to this delivery kind.
Read-only work still needs the requested result, applicable acceptance evidence,
real mailbox consumption, owned-resource cleanup, and normal task close.
An ACK, summary, dispatch receipt, or `--no-candidate` flag alone is not completion.

For code delivery, review acceptance creates the applicable daemon-owned
pending merge and notifies the current master. The obligation is durable and
appears in `collab context`, `collab status --all` (`pending_merges`),
`appsdk longhorizon show` (待合并), and master idle wake text. The exact
delivered candidate must be reachable from `refs/heads/main`; `task close`
fails with `TASK_MERGE_PENDING` while the applicable master obligation remains.
Never infer read-only work from a missing candidate. Never rely on remembering
a merge from a message.

After delivery, either the peer merges and pushes the verified code candidate
under the next section's conditions, or the current master drives review,
applicable integration, cleanup, task close, and then the next ready
assignment. A reviewed read-only delivery proceeds directly through cleanup
and normal close. Do not leave a peer idle merely because its last task
returned `delivered`, `merged`, or a review verdict. Route the next
non-overlapping P0/P1 work peer-first. Reuse an autonomous peer for another
long-horizon goal. Dispatch one-shot execution or observation directly to
independent GCM workers. Closing or force-closing a stale task is a scheduling
decision that must preserve evidence, not a reason to stop dispatching.

### Verified merge and resource cleanup DAG（单源单汇）

一个交付/resource 回收任务必须以唯一入口进入、唯一终点收口；节点内部可以是状态机，但独立功能之间不得跨节点回边改真源。

**peer 可直接 merge + push，当且仅当：**

1. merge 前 fetch 最新 `origin/main`，确认远端 main 没有比自己主链更新的提交；
2. 候选已 rebase/组合到该最新 main，并重跑过受影响验证 + 独立 review PASS；
3. 当前项目没有 pending merge/release 锁（简单 queue 文件或 daemon 状态）；
4. merge 后立即核对本地 main 与已验候选等价，并确认 `origin/main` 回执；
5. 任何失败（合并冲突、push 拒绝、CI 未跑通）都必须停下并上报 master，不允许强行推进。

否则 merge 的 owner 仍是 current master；review PASS 不授予流程外发布或生产变更。

### Collab runtime delivery DAG（单源单汇）

适用范围：Collab server、daemon、CLI、MCP、route、identity、notification、wake、
mailbox、task lifecycle 或安装包的修复/发布。源码测试和 `git push` 都不是终点；
runtime 节点未完成时必须报 `INCOMPLETE`。

```text
入口: latest_origin_main_candidate
  -> applicable_targeted_tests_pass
  -> scripts/install-global-collab.sh
  -> installed_binary_version_verified
  -> user_managed_runtime_activation_if_applicable
  -> collab_context_registered_live
  -> collab_mcp_initialize_matches_version
  -> live_replay_or_explicit_non_applicable_reason
  -> applicable_architecture_review_pass_after_behavior_verified
  -> commit_merge_push_verified
终点: cleanup_verified
```

daemon 生命周期由用户管理，agent 不执行维护动作、不索取凭据，也不为身份恢复
申请维护窗口。安装、源码测试与 live 验收分别报告；用户激活后验证受影响行为。

**peer 资源回收单源单汇：**

- 入口：本任务生命周期终点；
- 终点：本任务创建且确认不再需要的资源全部移除，并留下核对证据；
- 范围：worktree、playground、临时文件、日志、进程、forward；
- 核对：`git -C <repo> worktree list --porcelain` 不含本任务 worktree；`test ! -e <playground>` 成功；本轮 tmp/日志/进程已移除。

**master 资源回收单源单汇：**

- 入口：调度/交付收口；
- 终点：idle/stale 资源核销：过时 playground、失效 worktree、dirty main、已审查完成或明确 drop 的分支/候选/交付记录，逐个按 review/commit/drop 的授权终点核销并留证据；
- 边界：只回收已确认 stale 且有 owner 证据的资源，不得跨节点直接删除他人资源、共享 playground、共享进程或未经核销的候选；
- 失败：任何回收失败必须显式报告原因并标为未收口，不允许静默跳过。

巡检可以只读检查其他 owner 的 worktree、进程和 runtime，但检查不等于所有权：
不得删除他人资源或 worktree/branch，也不得终止共享 runtime。Collab 安装、重启、
注册、恢复和巡检都不得重启或停止 tmux server，也不得关闭或终止宿主 pane。

A peer orchestrates only its accepted scope, owns that goal's full lifecycle,
and maintains its own scoped patrol subscriptions. Each master assignment
requires a fresh scope assessment and an update to its existing patrol tasks
and subscriptions when the monitored goal changes. A legacy managed-subagent
record grants no new execution or patrol authority.
On a peer blocker, first ask the current master for help with the task,
evidence, attempted actions, proposed next step, and decision needed. Do not
delay the request while seeking a proven fix. If master is unavailable, record
it and continue safe in-scope work. If delivery or test conditions are unclear,
propose the missing conditions instead of guessing. The default direct-message
lease is daemon-owned and stays armed while the identity is registered;
finishing or closing the last task or a goal never cancels it, so a registered
peer stays wakeable. There is no subscription-closing command, interface, or
alias. When a finite goal or task patrol has no actionable work, end it through
its owner lifecycle (`appsdk goal cancel` for a goal, task closure for a
task-owned schedule); this never controls base direct-message reception.

AGY review is not used for Collab v1 lifecycle gates. Ordinary review uses an
independent review path when review is required; milestone review uses Codex
Review with the `oauth` profile and `gpt-6.1-sol`. Astra is not a reviewer.
Missing AGY is not a blocker because it is excluded; missing a
declared review gate is a failure.

Without an available, verified selected transport, initialization fails
explicitly. AppServer is preferred when its candidate passes the owner
self-check; a tmux-only binding remains a separate selected transport and is
never substituted for an AppServer binding. Read-only journal/mailbox queries
remain available where their command permits unregistered access. Pane output
is an observation, never task or control truth.

| Intent | Command |
|---|---|
| Bootstrap, recover, or re-locate this agent; read peers, master, scheduling state, and env | `collab context` |
| Notify a peer now | `collab sendmessage --to <peer> --subject <short-topic> "<original message>"` |
| Receive and consume notifications | DSH: native full-body delivery by gateway; other transports: `collab recv` |
| Read one notification without consuming | `collab msg <notification-id>` |
| List unread messages | `collab inbox` |
| Recover an already-delivered notification | Non-DSH only: `collab ack <id>` or `collab ack --all`; DSH gateway owns Ack |
| Inspect worker health and notification status | `collab worker status [id]` |
| List peers (operator diagnostic; also in `collab context`) | `collab who` |
| Check own subscriptions | `collab notify status` |
| Inspect current master (operator diagnostic; also in `collab context`) | `collab master status` |
| Replace the current master with an explicitly approved promote | `collab master promote --approval "<user text>"` |
| Clear the current master authority with explicit approval | `collab master clear --approval "<user text>"` |
| Delegate master authority to another peer (current master only) | `collab master delegate <peer>` |
| Split work to a registered peer | `collab sendmessage --to <peer> --subject <topic> "<assignment with delivery and test conditions>"` |
| Request blocker verification from the current master | `collab sendmessage --to <master> --subject blocker "<task_id; failure evidence; attempts/results; alternatives; decision/resume condition; cause/fix if known, otherwise unknown>"` |
| End your own finite goal schedule | `appsdk goal cancel` (task-owned schedules end with task closure) |

After a transport preview, use its notification ID and abbreviated subject to weigh
urgency against the current task. When selecting the notice, run
`collab msg <notification-id>`, read durable detail, and execute the actionable
request inside this Agent's scope. Do not stop at ACK or waiting; mailbox truth
persists.

### Situation -> action (one entry)

This table applies to registered participants recovering state, or to initial
registration explicitly requested by the user. It does not activate Collab
merely because an agent enters a project.

`collab context` is the single agent entry. It resolves the canonical root,
connects to the existing daemon, creates a missing baseline when applicable,
restores peer identity, re-arms the default direct-message lease, and returns the
authoritative snapshot plus the current role's `operations`.

That one snapshot is also the agent's complete state read. It carries the
projections that used to require separate calls, so no agent flow needs to run
`collab who`, `collab status --all`, `collab master status`, or a shell
`env | rg` probe first:

| Snapshot field | Replaces | Contents |
|---|---|---|
| `master` | `collab master status` | current master authority projection (holder, scope, approval) and `master_wake`; transport liveness is a separate fact |
| `peers`, `peer_count` | `collab who` | every registered peer with role and presence |
| `summary`, `master_wake`, `subagents` | `collab status --all` | worker/message/task/subagent counts and the scheduling state |
| `pending_merges` | `collab status --all` | applicable durable code-delivery merge obligations; explicit read-only delivery has none |
| `env` | `env \| rg '(COLLAB\|APPSDK\|CODEX\|HOME\|USER)'` | the identity-relevant variables of this agent's own process; credential-shaped names (`*TOKEN*`, `*KEY*`, `*SECRET*`, `*PASSWORD*`, `*CREDENTIAL*`) are dropped, so `CODEX_API_KEY` never appears |
| `operations`, `next_actions` | — | the current role's required actions |
| `requires_identity_update` | — | present only when the agent must repair its identity; see below |

| Situation | Do this | Never do this |
|---|---|---|
| User explicitly requests first registration | `collab context` | automatic registration merely on entering a project; a separate identity probe, or `collab init` used as a substitute for `collab context` |
| Another worker or project claims your tmux pane, or the recorded pane route is stale | `collab context`; the daemon replaces the claim by default | inspect routes or archives, pick a worker id, or edit any route/identity file |
| Master authority must move to this peer | `collab master promote --approval "<user text>"` | promote without explicit user approval |
| Master authority must be cleared for this project scope | `collab master clear --approval "<user text>"` | clear without explicit user approval; or expect clear to delete tasks, messages, peers, or bindings |
| Thread/session changed, or after daemon restart | `collab context` (daemon reconciles identity in place) | manual identity or daemon lifecycle commands |
| Worker status, board admission, or a mutation reports a stale binding generation or `SESSION_THREAD_BINDING_MISMATCH` | preserve the registered binding and run `collab context` as the only repair entry | manually rebind, edit routes or journals, or infer a binding |
| Need peer list, master state, scheduling state, or env | read them from the single `collab context` snapshot | call `collab who`, `collab status --all`, `collab master status`, or grep the environment as a separate step |
| `requires_identity_update` is present | supply only requested real runtime facts; a user-authorized identity/name/role recovery uses the exact intent command in stable-name-recovery; context handles migration internally | guess archived workers, inspect routes/tokens, hand-edit state, invent approval or treat historical ambiguity as insufficient user authority |
| `collab context` exits non-zero with `COLLAB_CONTEXT_UNRESOLVED` | preserve the error; failed initial registration continues independently; existing registered participants report the affected coordination without clearing owned tasks/claims | automatically retry from another cwd; edit routes/token state, copy identity, reset the project |
| Default lease looks stopped | `collab context` re-arms it from the registered identity | probe sockets, call a transport directly |
| Notification arrived | DSH: act on the native full body and reply with `collab sendmessage`; other transports: run `collab recv` once, then act | DSH running `recv`/`ack` or recovery before replying; treating submission as consumption |
| Master has a pending merge | `collab context` → `pending_merges`, merge the candidate, then `collab task integrated` | rely on a remembered message, or try to close first |

### Master peer CRUD

`collab context` returns a `peer_lifecycle` operation card only to the current
master. It contains the exact commands, scope rule, success receipt, and failure
handling. Use it as the command contract. A peer with no current master grant
does not manage other peers.

All targets must belong to the same registered canonical project `main` and app
scope as the master. The requested `cwd` may be the main checkout or any
existing directory/linked worktree in that same project. Collab resolves the
worktree to its registered `main`; directory nesting or a matching path prefix
does not establish project identity.

| When | Command | Required result |
|---|---|---|
| Create a peer | `collab worker create <peer-id> --cwd <existing-project-dir-or-linked-worktree> --op <stable-operation-id> [--model <model>]` | `outcome=complete`; response identifies the real thread, route, registration, and verified ready turn. Reuse the same operation ID to recover a retry; never create a replacement ID after an unknown result. |
| Inspect a peer | `collab worker read <peer-id>` | `outcome=ok` with the exact peer lifecycle projection. `collab worker read` with no ID reads the caller. |
| Move a peer's AppServer cwd | `collab worker update <peer-id> --cwd <existing-project-dir-or-linked-worktree>` | First call omits `--op`. `outcome=complete` requires settings update and cwd readback. Supply `--op <returned-operation-id>` only to retry/query that retained update after an uncertain result. |
| Unregister yourself | `collab worker close --self --reason "<auditable reason>"` | The authenticated current pane may retire its own identity even with outstanding tasks. `outcome=complete` requires self binding and route retirement; `runtime_archive=not-applicable`. The host process, tasks, messages and history are preserved. Only this identity's exact master grant is revoked. |
| Close another peer | `collab worker close <peer-id> --reason "<auditable reason>"` | First call omits `--op`. `outcome=complete` requires verified runtime archive and peer, binding, route, lease, and subscription retirement. Open responsibilities are refused. Use `--op <returned-operation-id>` only for its retained retry/query. |
| Read an uncertain operation | `collab worker query --op <returned-operation-id>` | Reads the retained outcome without repeating a host effect. Do not start a replacement operation while the result is unknown. |

For self-unregister, the first call also omits `--op`; use the returned operation
ID for query or retained retry. Self-close does not stop the host process or
delete tasks, messages, claims, pending merges or history. Unread messages
remain durable. A later `collab context` at the same live anchor rebuilds a
default peer with a current binding and route. It does not restore the retired
master grant.

### Peer observation

Any authenticated peer of the same registered project may read the latest live
buffer of an exact registered tmux peer. Master authority is not required
because the read changes no durable state.

| When | Command | Required result |
|---|---|---|
| Decide whether to keep waiting, escalate, or steer | `collab snapshot <peer-id> --lines <1..200>` | Returns the latest bounded, transient tmux pane buffer through one presence check and one `capture-pane`. It sends no input, consumes no mailbox message, bumps no revision, and writes no durable snapshot receipt. AppServer and DSH peers remain unsupported. |

`collab worker snapshot <peer-id> --lines <1..200>` remains a compatibility
alias. The buffer is only the latest visible pane text. It does not prove
busy/idle, Codex turn state, message consumption, or task completion.

For `refused`, `partial`, or `unknown`, keep the typed response and its
operation ID. A responsibility or project-scope refusal has no lifecycle
success; fix the named condition before a new operation. An unknown or partial
result is not success: query that operation and do not claim the peer was
created, updated, or closed until its verified terminal receipt says so.

Read-only operator diagnostics remain available for human audit: `collab who`,
`collab status --all`, `collab worker status`, `collab
route resolve`, and `collab master status`. They are not agent identity
recovery steps. User daemon maintenance is outside the agent interface.
`collab init` is not an agent bootstrap and not a second identity algorithm: it
drives the same daemon identity gate as `collab context`, and it remains the
documented entry for existing AppSDK init consumers. Historical pane claims
do not override the current cwd/session or block peer
restoration. Optional user authorization replaces the current project master
through the existing authority owner.

### Context 状态与终点（DAGpipe：单源单汇）

`context_request` 是唯一入口，`state_snapshot` 是本次权威结果出口。现有七节点链为：
解析项目与控制范围 → 读取现有服务 → 读取持久名字、agent sessionID 与占用 →
判定续接、分配、确认或待决 → 提交名字绑定并同步未关闭任务或持久待决 →
生成安全环境投影 → 输出含名字归属的权威快照。binding 收敛属于现有名字提交链，
由既有 credential/binding/route owner 完成，不另建身份选择路径。
Binding convergence makes context completion, worker status, and board admission
read one authoritative project/app/agent binding and endpoint generation. Only
context repairs; status and board admission are read-only consumers. Missing,
ambiguous, stale, or principal-mismatched bindings return a typed
`SESSION_THREAD_BINDING_MISMATCH` with known expected/observed generations and
`collab context` as the only repair entry.
The canonical project and app scope bound the lookup; another project's binding
or a historical pane/native route cannot supply a missing current binding.
Endpoint liveness remains separate from durable identity validity.

Missing facts are the terminal of this invocation: `registered=false`,
`identity=null`, and `requires_identity_update` gives exact `required_fields`,
`reason`, `action`, and `exact_error`. The agent uses one
`collab context --provide` for this invocation's requested current facts; native
facts are required on every new context call, even after registration. Initial
registration does not wait for challenge. Root failures and explicit
conflicts preserve the original error and do not masquerade as a successful
snapshot:

| 状态/终态 | 含义 | Agent 动作 |
| --- | --- | --- |
| `state_snapshot` 且 `registered=true` | 注册成功；含 role/operations/master/peers/peer_count/summary/master_wake/subagents/inbox/worktrees/tasks/env | 读快照执行已注册角色的 `operations`；无需再跑 who/status/master status |
| 历史 pane claim 与当前投递位置不符 | daemon 对齐通信位置；不由 pane 决定名字归属 | 名字续接按稳定名字合同；在线他人占名需确认，unknown 交 Jenny |
| 名字占用 unknown | 持久 context operation 返回 `pending`；不是注册成功 | 等待同一请求裁决续接；Jenny 自身 unknown 且本次用户明确恢复并确认接管时直接接管 Jenny/master |
| `identity_update` | daemon 缺少真实运行事实；`registered=false`，`requires_identity_update.required=true` | 按 `required_fields`/`reason`/`action` 只补实际缺项。已授权恢复意图按稳定名字合同提交；重复档案及旧绑定由 context 自行迁移。若仍泄漏裸历史冲突，保留原错作为实现缺陷，不让 agent 选旧身份或维护控制面 |
| 可选通信诊断 pending | 原身份与当前 route 仍可用；诊断 reply 未完成 | 该诊断不是恢复前提；若显式使用，participant 从所选 adapter 读取 challenge 并从自身环境回传；`--query` 保持只读 |
| `COLLAB_CONTEXT_UNRESOLVED` | 无 route、无 baseline、无 git 根（非零退出） | 保留原错，不自动换 cwd 重试；首次注册失败则独立继续，已注册者报告受影响协作，保留已领任务/claim |
| 拒绝在 playground 创建基线 | 在 worktree 内引导（非零退出） | 保留原错，不改主树或删除旧身份；首次注册失败则独立继续，已注册者保留已有职责并报告协作缺口 |
| 默认订阅已停 | daemon 按当前注册身份自动重武装，不存在显式退订 | 需要再收消息时运行 `collab context` 自动恢复，不由 agent 手工重订阅 |

`requires_identity_update.reason` is a typed code. It is not prose and it is not
permission to select a worker. The supplement has exactly the four scalar fields
`session_id`, `thread_id`, `endpoint`, and `namespace`. A value that conflicts
with an automatically observed fact is `IDENTITY_FACT_CONFLICT`, not an override.
Historical duplicate identities and retired carriers are context-owned migration
inputs, not conflicts for the agent to resolve. Current user-approved recovery
is evaluated before those records. Real input conflicts, token rejection, route failure, runtime-binding failure, and
transport failure keep their original error and non-zero exit; unapproved
automatic resolution cannot bypass them. A communication diagnostic can await
a participant return; ordinary identity restoration does not.

完整语义图、转移表和 owner 映射见 `docs/collab-context-state-machine.md`；
机器可校验 SESE 图见 `docs/dagpipe/collab-context.graph.json`
（`dagpipe graph validate`）。

## Initialize once

Only for an existing registered participant or an explicit user request to
register, the single state entry is:

```sh
collab context
```

The daemon-owned identity context completes the bootstrap. It reconciles
delivery placement inside this call. Pane history does not select the name;
same-name continuation, online takeover confirmation, and unknown adjudication
follow the stable-name contract.
`collab init` is not a second agent entry and not a second identity algorithm,
but it drives the same daemon identity gate and stays the documented entry for
existing AppSDK init consumers. Neither `collab init` nor AppSDK init creates,
changes, or clears master authority; init is not an implicit authority reset.
Agents do not run a separate identity, route or archive probe, or a manual
subscription, as part of bootstrap.

## 三种宿主的注册方式与锚点（DSH / Codex TUI / Codex Desktop）

注册入口对三种宿主相同：`collab context`。宿主只决定锚点与要报告的事实，
不决定角色：任何宿主注册后默认是 peer；成为 master 只能由用户明确授权
（`collab master promote --approval '…'` 或 `collab context --approve-grant`
action=restore_master），或由已授权 master 委托。注册前先确认自己运行在哪种
宿主，再报告该宿主的真实事实，不要伪造或复用旧值。

| 宿主 | 身份锚点 | 注册时报告的事实 | 不许做的事 |
| --- | --- | --- | --- |
| DSH | gateway session（`DSH_SESSION_ID`）+ communication API（gateway 控制 socket 与 runtime id） | 普通 `collab context` 由 CLI 读取 gateway 发布的控制能力并查询该真实 session 的当前事实；已有注册沿已提交 route 恢复。`--provide` 仅用于显式提供真实 DSH 能力 | 不手工猜测或从日志、plist 找 ids；不填 Codex thread/namespace；不与 appserver/tmux 候选混报；不用 tmux pane 当锚点 |
| Codex TUI / CLI | AppServer session + thread，或当前 tmux pane | `session_id`、`thread_id`、`endpoint`、`namespace`；CLI 自动读 `CODEX_SESSION_ID`、`CODEX_THREAD_ID`、`TMUX_PANE` | 不假设自己一定有 tmux pane；不填 pane 冒充 Codex 会话；不借用其他进程的 AppServer endpoint |
| Codex Desktop | AppServer session + thread，经受管 MCP bridge | 同 TUI 字段；bridge 的 executor metadata 提供真实 caller thread/turn，daemon 只连该 bridge 的 UDS | 不连 tools pipe，不猜测不存在的 AppServer endpoint，不把 recipient target 当 caller |

锚点归属规则：一个宿主只有一个锚点；DSH 的锚点是 gateway session 加 gateway
控制面标识，不是 tmux pane，即使该 DSH 会话在 tmux 里启动，pane 也只定位
观测、不承载身份。普通身份恢复只读持久身份与绑定，不重新 Register、不回环、
不要求通信可达；只有显式 `--recover-communication` 才做通信回环验证，那是
通信诊断、不是恢复前提。同一 session 出现在多个 API 上时，daemon 用 session
加 communication API 联合判定；仅 session 相同会显式冲突失败，不会静默覆盖
已有身份、角色或工作。DSH 首次注册只需普通 `collab context`，CLI 自动查询真实
gateway 能力。缺控制声明属于 gateway 集成缺口，应保留明确错误，不让 agent
查文件、猜 ids 或手工补值；显式 `--provide` 仍须来自真实能力。详见 AppServer runtime
registration 与 DSH 段的验证规则。

## AppServer runtime registration

Register only the AppServer that serves the current caller's active session and
thread. The endpoint, session ID, and thread ID must come from that caller's
own runtime; never submit another agent's or worker's AppServer, session, or
thread, even if it is reachable. If ownership of the exact endpoint and tuple
cannot be established, fail registration rather than borrowing another
runtime's identity.

Registration derives the host namespace from the active runtime. The exact
`CODEX_INTERNAL_ORIGINATOR_OVERRIDE=Codex Desktop` marker selects `codex_app`;
`Codex CLI` or `Codex TUI` selects `codex_tui`, and `TMUX_PANE` selects
`codex_tui` when no originator marker is present. An explicit
`COLLAB_APPSERVER_NAMESPACE` is reserved for nonstandard hosts and must be one
of those two values. Unknown host markers never select an AppServer namespace.
With an explicit AppServer socket they fail registration; without one, the
adapter returns no AppServer candidate and may continue through another
existing transport only after that transport's normal verification. Never
default an unidentified session to `codex_tui`.

For the AppServer branch, the Collab client accepts only `COLLAB_APPSERVER_SOCKET` or
`CODEX_APP_SERVER_SOCKET` supplied by the current runtime as the endpoint for
the current session. In particular, never derive
`$CODEX_HOME/app-server-control/app-server-control.sock` or
`$HOME/.codex/app-server-control/app-server-control.sock`: those are
host-managed paths, and their existence, owning daemon PID, or ability to list
the thread does not prove that the registering client uses that AppServer.
Never copy a parent agent's, another process's, or a merely host-managed
endpoint into registration or `collab context --provide`. A matching
`thread/read` result proves that an endpoint can access the thread; it does not
prove that the current client uses that AppServer. If the current runtime does
not provide its endpoint, do not register an AppServer route or guess one; use
another already verified transport only when it belongs to this peer, otherwise
report that registration lacks a current-session endpoint. `CODEX_APP_TOOLS_PIPE_PATH`
is a tool pipe, not an AppServer endpoint. Desktop uses its separate normally
managed MCP bridge: executor metadata supplies the true caller thread/turn,
and the bridge publishes its scoped local capability to context. The daemon
connects only that bridge UDS; it does not connect the tools pipe or require
a nonexistent AppServer endpoint. The original real caller context remains
separate from the registered recipient target and the message sender. See
[current recovery facts](references/stable-name-recovery.md).
The daemon still verifies the exact thread/session/project tuple before
committing a supplied route.

The AppServer namespace is persisted with the recipient route. Its `turn/start`
notifications use that stored recipient namespace (`codex_app` or
`codex_tui`); they never infer it from the sender or hardcode the TUI namespace.
If an AppServer endpoint is unavailable before registration, the existing
transport selection rules apply. Once an AppServer binding is selected, do not
switch transports after an RPC error.

## Worktree identity

A Git worktree is a task execution directory, not a second identity or a
substitute for the canonical project root. A peer and its master are related
through the same registered canonical project `main` and app scope. Their
worktrees may be in different directories. `collab context` resolves the
canonical root automatically and the daemon owns identity selection, creation,
recovery, update, route publication, and binding persistence. Do not create a
worktree-local peer. External linked worktrees resolve to their registered Git
main root for context and ordinary commands; path containment does not establish
project identity. Do not switch cwd to recover an identity. The `collab context` snapshot is the agent's complete
master-authority and peer read. A failed context is not evidence that no master
exists; preserve the exact error. Master promotion requires explicit user
approval and an authenticated current binding, not the incumbent's or the
candidate's liveness. `collab master status` remains a
read-only operator diagnostic.

### Native thread route resolution

The daemon is the sole route selector. The current AppServer owner plus exact
session/thread binding identifies a route. Historical routes and tombstones
are not candidates. The daemon verifies the selected owner with native
`thread/read`; a tmux socket + pane locates transport only. Name ownership and
the current agent sessionID determine continuation. Tmux never substitutes for
an AppServer communication channel. The daemon reads tmux route ownership
without an admission probe.

`collab route resolve` exposes a read-only operator diagnostic for the current
native thread; `--native-thread-id <id>` and `--session-id <id>` must match the
live runtime binding. It is not an agent identity recovery step. The daemon
returns exactly one route, `ROUTE_RESOLVE_NOT_FOUND` for zero matches, and
`ROUTE_RESOLVE_AMBIGUOUS` when multiple current bindings match. Missing, unknown,
mismatched, or malformed endpoints fail explicitly. The resolver is read-only
and returns no token.

## Subscribe to a future event

Use subscriptions only when this Agent wants a later event to wake it:

```sh
collab notify subscribe --event resource-released --subject <resource-id> \
  --ttl-seconds <bounded>
collab notify subscribe --event deadline --subject <timer-id> \
  --ttl-seconds <bounded>
```

The sender never inspects or configures the recipient's subscription.
For subscription semantics or delivery diagnosis, read
[references/notifications.md](references/notifications.md).

Goal registration belongs to the current master or peer's own accepted scope,
independent of Desktop/TUI placement. Host type does not grant or deny a role.
Unregistered bounded subworkers and legacy managed-subagent records cannot
claim an independent goal or infer master authority.

## Hard boundaries

- Never attach production work to v2 or `.agent-collab-v2`.
- Project scope comes from the exact process cwd. Never choose/search/hardcode
  a path. MCP and child commands inherit the same environment.
- Identities are equal peers by default. There is no implicit master from
  first registration, automatic process recovery, or inferred `/goal`. Collab
  master is explicit, user-approved project arbitration; it is not Codex root,
  and it does not take ownership of another peer's task. The authority owner is
  the current typed grant in the exact project and app scope, with only empty
  or assigned states; the current holder controls independent of its transport
  liveness. An explicitly user-approved `collab master promote` replaces the
  recorded holder, and only the current master grant holder may
  `collab master delegate`. Independent peers may
  decline a master collaboration invite. Master splits by dependency then
  unique write scope, offers long-horizon work to a consenting peer through the
  public board with unambiguous delivery and test conditions, and keeps
  architecture/integration/acceptance; it does not take another peer's task.
  Peers request help promptly with available failure evidence and attempts;
  root cause and solution need not be proven first. Apply Registered patrol
  duties for master verification and continued ownership. Legacy managed records grant no new task
  authority. Last owned
  task close keeps that owner's auto-notify armed. There is no subscription-closing
  command; end a leftover finite goal schedule with `appsdk goal cancel` and
  keep base direct-message reception daemon-owned. AGY review is not a Collab v1 gate; ordinary review is independent,
  and milestone review uses Codex Review (`oauth`, `gpt-6.1-sol`). Legacy
  managed records remain diagnostic compatibility facts only.
- Each peer owns its complete task/worktree/integration/resource/cleanup
  lifecycle. Never mutate or close another peer's work.
- Send requested replies, authorized action results, explicit notices,
  shared-resource coordination, or subscribed async results. Do not send
  unsolicited routine progress, heartbeat or ACK messages.
- A wake is only a signal. It cannot change task/resource truth, fabricate
  success, authorize external actions, or create an ACK loop.
- `absent` or `unknown` liveness for the selected binding produces no transport
  input. If its endpoint is unreachable, unowned, or mismatched, the
  subscription enters its explicit unavailable state to prevent storms. There
  is no cross-transport fallback after binding selection.
  Each due batch is reserved durably once; failed or uncertain attempts are
  never automatically replayed, including after restart. Details remain
  readable in the inbox.

## Load details only when needed

- Task/worktree registration, delivery, close, cleanup:
  [references/task-worktree-lifecycle.md](references/task-worktree-lifecycle.md)
- Resource conflicts and bounded waits:
  [references/resource-waits.md](references/resource-waits.md)
- Migration, user-managed daemon boundary, deprecated commands:
  [references/migration-daemon.md](references/migration-daemon.md)
- Source/release/install/live verification:
  [references/verification.md](references/verification.md)

Do not load references for ordinary `sendmessage`, `msg`, or `inbox`.

The current v1 contract has no 15-minute periodic worker liveness. Worker and
master liveness wake only on a supported timer/wake, direct message, or real
external event. If any installed copy still contains the old wording, replace
it from this SKILL and its references.
