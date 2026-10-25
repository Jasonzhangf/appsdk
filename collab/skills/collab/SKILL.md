---
name: collab
description: "仅约束已成功注册 Collab 的 peer；master 权限需另行授权。未注册、注册失败或无法注册时独立完成，不适用协作协议。用户明确要求注册时才初始化。"
---

# Collab

## 唯一 Skill 与当前工作流程

Collab 只有一个 Skill。master、peer、designer、reviewer 等不建立独立 Skill。
源码 `collab/skills/collab/` 和安装目录 `~/.agents/skills/collab/` 是同一 Skill 的源码与
安装产物；宿主入口链接到该安装目录。流程模板属于本 Skill 的资源，不是另一个 Skill。

标准流程唯一资源为 [workflow-templates.json](references/workflow-templates.json)，
基础版本为 1，只有 `master`、`peer` 两种身份模板。各自管理自己的目标、任务和执行
流程，优先将有界执行交给独立 worker。peer 完工或停止前向现有 master 交回结果并申请
下一步或 idle；reviewer/设计师同样适用。无 master 时不虚构审批者；master 不向自己申请。

执行以本次 `collab context` 的 `role_brief` 为准：`workflow_template` 只提供基础版本、
identity 及 specialties 的来源与 revision；展开后的 role_task、responsibilities、
next_action、completion_action 从 `role_brief` 顶层读取，每个职责只出现一次。
未更新时使用标准身份流程。
项目 master 可覆盖身份流程，或配置 `role:<工作角色>` 流程；多个匹配专长全部有序返回，
不以第一项静默覆盖其他项。角色被移除后，不再派发该专长模板。模板不是权限授予，
也不自动改写已发布任务的 owner、scope、指定执行者或必需 reviewer。

每次获知新 peer 或角色关系变化，master 在已有授权内更新受影响模板、任务分工和
交付接力，实际安排 ready 工作，再向用户报告生效规则并继续；用户可随时修改。
报告采用：变化；当前分工与 scope；交付/review/后续安排/idle 负责人；本次已派任务
或无可派原因；当前生效及可修改说明。只有超出授权、改变用户显式硬约束或关键事实
不清时才确认受影响部分。更新正在执行的任务时，必须显式落实交接。

manager（全局 CEO 或项目 master）为任何承担计划、编排或任务派发的角色创建或更新
流程模板时，必须在该角色的 `responsibilities` 中写明：计划与派单要明确目标、范围、
非目标、权限、依赖、验收条件和交接负责人；执行者须在约定边界内投入完成任务所需
的资源，不得为省预算删减必要工作或验证，不得越界，也不得假定别人会救火。
资源不足或确需改变边界时，向派单 owner 提交证据并请求调整，只暂停受影响工作。
这一职责随下游规划、编排和派发继续传递；不授予无限预算或范围外权限。

模板更新沿用 `context`，仅当前项目 master 可执行，正文为四字段 JSON：

```sh
collab context --workflow-target 'role:designer' --workflow-template '{"role_task":"负责已授权设计工作","responsibilities":["核对当前任务范围与交付接收者"],"next_action":"推进当前设计任务并交给指定 reviewer","completion_action":"交回设计证据，向当前 master 申请后续安排或 idle"}'
collab context --workflow-target 'role:designer' --reset-workflow-template
```

`--workflow-target master` 或 `peer` 更新本项目的身份流程。`role:` 后为已规范化的
工作角色名，不是 agent 名。模板写入和角色设置分开执行；写入不会替别人注册或分配角色。
修改后同次回执及后续 context 均返回当前投影，`operations` 使用同一有效流程。
模板内容由 daemon journal 持久保存，换会话不丢失；重置模板后回到标准身份流程。
若实际 context 没有该模板字段，说明当前运行版本尚未提供模板派发；保留原回执并报告
安装/运行差异，不手工改控制状态、不虚构模板已生效，也不自行管理 daemon 生命周期。
已运行的 peer 需要立即处理变化时，master 通过已有消息/派单通道通知其读取 context，
不新建定时轮询。context 派发流程不等于 daemon 自动生成任务或替 agent 执行。

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
| 确实跨项目 | 沿 [master-to-master 通道](references/migration-daemon.md#cross-project-master-communication)；普通 peer 交给自己的 master |

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
跨项目消息的 `from` 是 `<worker-id>@<source-project>`；必须按
[Cross-project master communication](references/migration-daemon.md#cross-project-master-communication)
走 `master send`，不能把整个跨项目地址传给 `sendmessage`。
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
落实当前可执行动作；确无可继续工作时，按当前流程交回并申请下一步或 idle。
完成后回复尚待外部条件时保留标记并说明
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

### 指派前分析并保存

master 或负责子目标的 peer 在指派前先分析 scope、交付与验收及角色要求。master 在
上述自动安排授权内保存分工、派发并报告，不逐轮等待用户批准。复用用户已明确要求；
只有超出授权、改变用户显式硬约束或关键事实不清时，才确认受影响部分，其余工作继续。
未指定固定执行者或 reviewer 是允许的，不将可选字段为空当作必须补问的缺口。

角色用 `collab context --work-roles` 按项目和稳定名字持久保存；分工用 `collab board policy` 与
任务明确字段保存。scope、交付与测试条件写入已有 board 任务的 description、
delivery-condition、test-condition，适用授权原文或所在对话位置及安排依据记入任务描述。
角色及执行者约束以正式控制字段为准，不能只留在聊天、消息正文或 agent 临时记忆。
不新增第二份角色表或审批状态。保存后读取任务回执，确认范围和有效分工符合用户要求。

同一已授权 scope 内的后续请求沿用这些要求；master 自动更新默认分工并报告。
超出授权或改变用户显式指定的 scope、执行者、reviewer 时，再确认受影响部分。
普通进度更新、重新唤醒、换会话不构成重新确认的理由。

工作角色可动态新增、修改或撤销。用户明确提出的角色变更就是该次确认，不重复请示。
`context --work-roles` 设置完整角色集合：新增时保留仍需要的旧角色，删除时省去对应
角色，全部撤销时传空字符串。修改后读取回执，后续分派和每次接单读取最新持久角色，
不缓存注册时的旧角色。角色更新不自动改写已有任务的 scope、指定执行者或 reviewer；
需要同步变更任务时，按适用授权显式更新对应约束并落实交接。不能凭角色变化自行接管或放弃任务。

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
| 当前 master 保存用户要求或自动安排授权内的某类工作默认分工 | `collab board policy --scope project --work-kind '<类别>' --prefer-role '<角色>'` | 指定执行者时用 `--executor '<名字>'`，指定审查者时加 `--reviewer '<名字>'`；自动选择依据当前角色与任务，保留用户显式硬约束，并报告生效分工。 |
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
Collab 只有一个全局、由用户管理的 daemon；agent 不启动、关闭或重启它，也不为
身份恢复申请维护窗口。Collab 安装、注册、恢复、巡检或 daemon 不可达都不触发
tmux server 的重启、停止或宿主 pane 终止。用户维护命令、参数和凭据不属于 agent 接口。

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
for automatic runtime-service admission, commands and receipts. New chats need
no prior MCP invocation. Do not guess or reuse an AppServer socket.

DSH 的接收方式不同：gateway 独占 canonical `Poll` / `Ack`，将完整正文直接送入
原生会话。收到正文后直接处理并用 `collab sendmessage` 回复。不要运行 `collab recv`
或 `collab ack`；这两个 CLI 命令会返回 `DSH_NATIVE_DELIVERY_OWNED`，避免与 gateway
争抢信箱。需要诊断时只读 `collab msg <id>` 或 `collab inbox`。CLI 收件、消费与
ACK 指引只适用于非 DSH 传输。通信恢复也不是处理原生正文的前置步骤。
（[非 DSH 收件与消费](references/migration-daemon.md#desktop-cli-registration-and-messaging)）
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

The exact master and peer responsibilities are the role brief's `master` and
`peer` templates; registration and recovery must explain that goal level and
recommend scheduled patrols for each role's own goals, risks and resources.
The Collab-specific wire facts stay here:

- An ordinary peer keeps the code-compatible wire role `worker` and derivation
  `peer`; it does not inherit master authority.
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
rearming it, immediately continue all ready work; a future deadline, missing
wake, or unavailable patrol service never blocks that work. The role brief's
`idle_patrol_rule` owns idle and patrol behaviour, including when to stop, whom
to report to, and how to close idle scope; this section keeps only the trigger
and the Collab-specific constraints.

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

Registration returns a patrol reminder. On the next authorized activation after
registration, promotion, delegation, or recovery, update the existing scope;
context restoration does not fabricate board tasks, timers, or subscriptions.

This contract is project-agnostic. Role-specific sections elaborate it, but
they cannot replace it with project-specific paths, commands, issue bindings,
or implementation details.

Collab-specific constraints on top of the role brief and
[notifications](references/notifications.md#patrol-duties-and-finite-schedules):

- A `master` patrols only its own project. The patrol covers open P0/P1 work,
  real peer evidence, owned worktrees and stale resources, canonical issue
  intake, and currently supported monitored events. Use durable board, task,
  delivery, cleanup, and issue evidence; inactivity, heartbeat, and ACK prove
  neither progress nor completion. Close each P0/P1 with applicable evidence
  or park it with owner, cause, and unblock condition. Dispatch ready P0/P1
  work before ending the scheduling turn.
- The accepted peer is the sole delivery owner for its scope; delegation and
  requests for help do not transfer it. Each accepted peer partitions its scope
  into non-overlapping bounded scopes and dispatches bounded one-shot execution
  or observation directly to independent `codex exec --profile gcm` workers,
  whether or not other peers are busy. Investigate failures and verify
  subworker limitations in the real entry; change the in-scope approach or
  authorized executor instead of repeating unchanged failures. Planner and
  Reviewer remain independent roles. Neither role infers completion from a
  message, receipt, or report.
- Use Close-Before-Open/SESE at task-domain boundaries: prove the old owned
  worktree is closed before opening its replacement. Dirty, active, shared, or
  unknown resources are PROTECTED_HOLD. Preserve and report them. Do not run
  broad process kills. Do not perform daemon or tmux lifecycle actions,
  including starting, stopping, restarting, or terminating a host pane.
- Send each new production issue through the project's canonical intake and
  deduplication before repair or dispatch. Record evidence according to the
  owning project contract; project-specific error codes, wire status, and
  response planes stay in the owning project Skill.
- Continuous patrol uses existing goal or deadline one-shot wakes. Before a
  turn ends while scheduling continues, explicitly re-arm a consumed one-shot
  wake. A read, ACK, status summary, or dispatch receipt is not completion and
  does not renew a wake. Do not invent a periodic polling loop.
- Every dispatch states the goal, scope, authorized actions, resource owner,
  delivery and test evidence, report channel, and stop conditions. The
  receiver checks the contract against current responsibility, then accepts,
  rejects, or negotiates before work starts.
- Each owner updates only its own finite patrol scope on assignment and closes
  it with the task. Add only missing scope, do not duplicate timers, and report
  the resulting scope or the concrete reason no subscription change is needed.
  Keep base direct-message reception; close only owned patrol scope, never
  another goal's subscriptions. This housekeeping does not gate ready
  authorized work.

A legacy managed-subagent record creates no new patrol task, subscription,
dispatch, or execution authority; it remains diagnostic compatibility state.

## 稳定名字与旧版 display-label 迁移

项目名字由 daemon 分配和恢复，承载工作归属，不再只是可以重复的显示标签。
Jenny 对应项目 master；普通名字按固定池顺序分配，各项目独立。同名字恢复或换
session 时继承工作并同步未关闭任务，不能恢复 pane ID 默认名或阻止名字工作继承。
内部 worker ID 仍可作兼容索引，但不能代替名字归属。

旧版 `collab worker name` / MCP name 接口曾只修改 label，不能用旧接口的行为模拟
新名字接管。升级以当前 `collab context` 与名字恢复合同为准，读取其权威回执；
旧 runtime 未加载新实现时明确报告版本边界，不把改 label 当作恢复成功。

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

## Recovery and Reset

### Recovery

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

Ordinary persisted restore reuses the identity and restores its current
communication binding, route and default lease without another Register or
mandatory communication challenge. Task content, worktree and old pane
claimants do not gate restoration; historical pane identities and routes are
retired. Actual I/O or persistence errors remain explicit without daemon
lifecycle steps. Acceptance of a restored peer and a restored master requires
each to send, receive and reply through the current route. A durable send or
role alone does not prove message consumption. Tmux session, pane, socket and
server PID locate transport only; native thread and activity facts locate the
selected adaptor and never replace the stable-name contract. For the master
restoration, DSH capability, and explicit communication-diagnostic variants,
read [stable-name recovery](references/stable-name-recovery.md).

When the snapshot contains `requires_identity_update`, read
`required_fields`, `reason`, and `exact_error`. Supply requested current facts
from their real source in this invocation. Each AppServer context invocation
carries the current session, thread, endpoint and namespace; the daemon never
fills these facts from historical registration. Desktop uses the separate
bridge capability through CLI or the optional MCP entry, not this AppServer
supplement. The supplement accepts only `session_id`, `thread_id`, `endpoint`,
and `namespace`; it rejects any other key. An agent never supplies
`worker_id`, approval, token, generation, binding, or project scope.
Missing facts preserve durable identity and work but do not complete restoration.
`--query` only reads retained progress and needs no current endpoint.
See [current-fact recovery](references/stable-name-recovery.md) for the contract.

The returned snapshot is the complete state read: role, operations, peers,
master, scheduling, tasks, binding, and filtered environment. `binding` comes
from the daemon ledger when the caller has a unique binding.

If `collab context` fails explicitly, preserve the original error and affected
scope. Do not retry automatically, edit route/identity files, copy tokens,
start another daemon, or invent a fallback. Daemon lifecycle is outside the
agent interface. Ordinary identity recovery uses `collab context` only.

### Reset

Reset is explicit, destructive, and master-only. The current project master may
run `collab reset --project` from the current project only when the user
explicitly requests discarding that project's Collab epoch. It clears only this
project and never the global daemon. AppSDK `reset-governance` or
`appsdk init --fresh --discard-legacy` is not a Collab reset; never remove
`.appsdk/`, `.agent-collab/`, journals, tokens or bindings by hand. Full rules:
[Migration and User-Managed Daemon Boundary](references/migration-daemon.md#reset-is-explicit-discard-not-recovery).

A later change invalidates only the conclusions it can affect; revalidate those
affected conclusions only. For post-fix regression recognition and divergence
triage, read [verification](references/verification.md).

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

Each brief carries `role_task`, `responsibilities`, `next_action` and
`completion_action`, plus the `idle_patrol_rule` and other
`coordination_rules`. Read those fields instead of any role summary repeated
here; `managed-subagent` is legacy compatibility only.

The default identity is peer. Master authority is explicit and
user-authorized: an approved `master promote` replaces the recorded holder,
and delegation is accepted only from the current master grant holder and
records that handoff. Registration, a process, inferred `/goal`, or
`role_brief` never silently creates master authority.

On trouble, apply the help contract in Registered patrol duties. A role change
via `master promote` or `master delegate` returns the new master brief; the old
worker brief no longer governs that peer.
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
Read it without consuming messages or changing lifecycle with `collab board
show`. Any registered role publishes a pending task and claims it, or the
master offers it to one peer with the observed revision:

```sh
collab board publish <task-id> --title <t> --description <d> \
  --delivery-condition <d> --test-condition <t> [--priority p0|p1|p2|p3|p4]
collab board claim <task-id> --expected-revision <observed>
collab board invite <task-id> --to <peer> --expected-revision <observed>
collab board respond <task-id> --accept --expected-revision <observed>
collab board respond <task-id> --decline --expected-revision <observed> --reason "<text>"
collab board update <task-id> --expected-revision <observed> --status <s> [--next <step>]
```

Publishing queues work; an invitation does not transfer ownership; claim starts
one pending task in the caller's own identity. Never claim outside the user's
authorized goal or take another owner's working or blocked task. `collab
subagent dispatch` is legacy and grants no new production work; bounded
execution or observation goes to an independent `codex exec --profile gcm`
worker.

`collab dashboard` is a read-only loopback observer of the same board. Full
command grammar, revision errors, MCP tool names, dashboard capability rules
and failure handling:
[task board and dashboard](references/task-worktree-lifecycle.md#public-task-board-and-read-only-dashboard).

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

## Global CEO and cross-project master communication

CEO is a host-global typed grant, not a work role. Each project master keeps
responsibility for its own projects' goals, peer split, acceptance and delivery.
Only the daemon's typed CEO grant grants global authority; the `ceo` work role
does not. Cross-project requests from an ordinary peer go to its own project
master. Only the current master grant holder may use the master-to-master
channel, and the target must be that project's current master grant holder.
Never switch cwd to impersonate another project's agent, write a mailbox
directly, guess a session ID, or bypass a rejection through another host
channel.

Entry points:

```sh
collab ceo status
collab ceo send --project <target-root> --subject <topic> --reply-required "<body>"
collab ceo append-task --project <target-root> --id <task-id> ...
collab master send --project <target-root> --to <target-master> --subject <topic> "<message>"
```

Exact commands, workspace and bootstrap rules, reply handling, and the
"target project has no master" flow:
[Global CEO and project master](references/migration-daemon.md#global-ceo-and-project-master)
and [Cross-project master communication](references/migration-daemon.md#cross-project-master-communication).

Task liveness and escalation routing for both roles:
[task liveness and escalation](references/task-worktree-lifecycle.md#task-liveness-and-escalation).

## Master owns the outcome, not the excuse

The role brief's `master` template owns master accountability, goal
orchestration, and the peer-dispatch contract: the master is accountable for
the final result of every assigned task and is the escalation target, not the
worker's shelter. The master keeps architecture, dispatch, integration,
critical repair, and final acceptance; it does not author the mainline.
Collab-specific force-close and escalation constraints below are the only
authority the Skill adds beyond the brief.

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
- The human remains the only final authority for goals, money, irreversible
  risk, and version promotion.


## Master splits and assigns

The master compiles the goal into a dependency graph, then assigns by unique
write scope; it does not take another peer's task or worktree. The full
decomposition and idle-wake routing live in the role brief's `master` template;
this section keeps only the Collab-specific constraints and the two details
that the brief does not own. **Peer-first** dispatch is the Collab constraint:
offer long-horizon goals through the public board to suitable, consenting
peers, and dispatch bounded one-shot execution or observation directly to
independent `codex exec --profile gcm` workers regardless of whether other
peers are busy.

**Sovereignty and Backlog Priority** (master-only, not in the role brief):
- **No autonomous technical debt refactoring**: when assigned work completes
  and the fleet is idle, the master must not autonomously launch long-range
  technical debt refactors, speculative architectural rewrites, or unapproved
  work. It formulates a structured proposal and pauses for the human.
- **Autonomous Bug Tracking Backlog Resolution**: pre-existing issues in the
  bug system (`appsdk bug list --status open`) are authorized project work; the
  master pulls and dispatches open bugs in strict priority order
  (P0 > P1 > P2) to keep capacity saturated before suggesting closure.

**Unacknowledged Workers & Pane Diagnostic Closure** (master-only, not in the
role brief): if a worker fails to acknowledge notifications or stays
unresponsive across repeated dispatches, run `collab worker status <id>` and
use durable Collab state plus the selected binding's status source to choose
the next action. For an AppServer binding, query native thread state. A tmux
pane is address proof only and never proves the agent is alive or `Present`;
its liveness stays unknown, transport status cannot establish message
consumption without the receive receipt, and a tmux screen cannot establish
task completion, peer consumption, or Codex turn state. Do not infer those
facts or restart a shared runtime from a pane probe.

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

The default direct-message lease is daemon-owned and stays armed while the
identity is registered; finishing or closing the last task or a goal never
cancels it, so a registered peer stays wakeable. There is no
subscription-closing command, interface, or alias. When a finite goal or task
patrol has no actionable work, end it through its owner lifecycle
(`appsdk goal cancel` for a goal, task closure for a task-owned schedule);
this never controls base direct-message reception. If delivery or test
conditions are unclear, propose the missing conditions instead of guessing.

Review routing, model selection, and review backends follow the host
Plan/Review contract. Collab v1 keeps an independent reviewer requirement but
adds no model or provider table. Missing a declared review gate is a failure.

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
| Codex Desktop | 当前 agent sessionID；native thread 定位通信 | 普通 `collab context` 自动接入 runtime 共享 bridge，由 native host 核实 thread/cwd 并登记；不需要先调用 MCP | 不手填 socket 或执行 turn，不猜 AppServer endpoint；daemon 只连 bridge UDS |

锚点归属规则：一个宿主只有一个锚点；DSH 的锚点是 gateway session 加 gateway
控制面标识，不是 tmux pane，即使该 DSH 会话在 tmux 里启动，pane 也只定位
观测、不承载身份。普通身份恢复只读持久身份与绑定，不重新 Register、不回环、
不要求通信可达；只有显式 `--recover-communication` 才做通信回环验证，那是
通信诊断、不是恢复前提。同一 session 出现在多个 API 上时，daemon 用 session
加 communication API 联合判定；仅 session 相同会显式冲突失败，不会静默覆盖
已有身份、角色或工作。DSH 首次注册只需普通 `collab context`，CLI 自动查询真实
gateway 能力。缺控制声明属于 gateway 集成缺口，应保留明确错误，不让 agent
查文件、猜 ids 或手工补值；显式 `--provide` 仍须来自真实能力。详见
[AppServer runtime registration](references/migration-daemon.md#appserver-runtime-registration)
与 DSH 段的验证规则。

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
  `collab master delegate`. Independent peers may decline a master
  collaboration invite. Master split/dispatch duties are the role brief's
  `master` template; peer help and master verification are the Registered
  patrol duties contract. Legacy managed records grant no new task authority
  and remain diagnostic compatibility facts only. Last owned task close keeps
  that owner's auto-notify armed. There is no subscription-closing command; end
  a leftover finite goal schedule with `appsdk goal cancel` and keep base
  direct-message reception daemon-owned. Review routing and model selection
  follow the host Plan/Review contract; Collab adds no model table.
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
  (also public board commands, dashboard, and task liveness/escalation)
- Resource conflicts and bounded waits:
  [references/resource-waits.md](references/resource-waits.md)
- Migration, reset, user-managed daemon boundary, peer lifecycle CRUD, CEO and
  cross-project master channel, AppServer registration, deprecated commands:
  [references/migration-daemon.md](references/migration-daemon.md)
- Subscriptions, notification delivery, and patrol schedules:
  [references/notifications.md](references/notifications.md)
- Explicit recovery variants and context terminal states:
  [references/stable-name-recovery.md](references/stable-name-recovery.md)
- State paths, transport selection, and read-only route diagnostics:
  [references/state-paths.md](references/state-paths.md)
- Source/release/install/live verification:
  [references/verification.md](references/verification.md)

Do not load references for ordinary `sendmessage`, `msg`, or `inbox`.

The current v1 contract has no 15-minute periodic worker liveness. Worker and
master liveness wake only on a supported timer/wake, direct message, or real
external event. If any installed copy still contains the old wording, replace
it from this SKILL and its references.
