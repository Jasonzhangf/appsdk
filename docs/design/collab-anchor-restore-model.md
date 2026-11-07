# Collab 身份恢复：当前项目与 session

本文件是身份选择的唯一设计正文。2026-11-04 的用户决策是：TUI 的稳定身份键是首次成功注册后固定在 tmux session 上的角色注册名；首次绑定把当前 tmux session 改名为该名字并读回确认，之后按这个固定名续接。`tmux_session_name` 只能来自 daemon 对当前进程的 live observation，caller-controlled environment 不进入名字决策；live pane 无法读取固定名时显式 fail-closed，不能按 sessionID 或 pane 猜测。pane、socket、`$N`、PID 和调用 cwd 不先于当前名字或 sessionID 选择身份；当前固定名与 sessionID 都不命中时，daemon 观察到的 pane 与调用 cwd 只有共同命中同一作用域 typed 占用才作为最后一级旧名恢复，单有 pane 或单有 cwd 都不恢复。Codex Desktop 按 agent sessionID、DSH 按 gateway sessionID 绑定，两者不执行 tmux 改名。实现入口是 `identity_resolver.rs::resolve_for_daemon_with_route_at` 与 `name_resolution.rs::plan_name_resolution`，提交仍由现有身份、名字和 master authority owner 完成。

## 默认 context

2026-11-05 用户纠正物理命名：`<实际调用cwd末级目录>_<Collab名>`，例如 `appsdk_Jenny`、`routecodex_Jenny`。tmux 不支持点号分隔，用户已确认下划线。`name_resolution` 是格式化和解析的唯一 owner。逻辑占用、任务和邮箱仍使用 `Jenny` 等逻辑名。只解析当前 cwd 的精确前缀；外项目前缀不提供本地名字候选。合法旧裸名可用于恢复，但必须迁移为带前缀物理名。direct promote 使用注册 worker.cwd。换名只处理当前 session 或有当前 typed 归属的旧持有者；无归属的裸 Jenny 不作为共享 server 的交换目标。

`collab context` 用当前 cwd 解析 project，用 daemon 观察到的固定 tmux session 名决定 TUI 名字意图。cwd 先参与项目解析，再与 pane 一起作为最后一级联合恢复锚点；历史 pane 绑定到哪个 worker 或 project 都不能改变当前 project，也不能阻断恢复。已有 worktree 的项目映射可用于解析 cwd；worktree 中的任务和工作内容不参与身份准入。

自动名字决策按固定顺序执行：显式 intent、TUI 固定 tmux 名的唯一 typed 占用、同 sessionID 的唯一 typed 名字、当前 master 的 Jenny、同作用域 pane 与 cwd 的共同匹配、普通池。较低优先级的旧 session 归属不否决已经命中的当前名字；不读取旧 `worker_id`、socket 或 `$N` 继承名字。

TUI 固定名有唯一 typed 占用时续接该名字；agent sessionID 变化时更新占用的 sessionID、binding 和不影响其他 owner 的未关闭任务与 mailbox。固定名无占用但同 sessionID 有唯一旧名时，按旧名修复并 rename；固定名与 sessionID 指向不同在线名字时以当前固定名为准，旧 session 归属不否决。固定名与 sessionID 都未命中时，只有同一 pane 与同一调用 cwd 共同命中一个同作用域 typed 占用才恢复该旧名；多个匹配或无匹配时继续普通池，不按顺序猜。当前 tmux session `$N`、socket、server PID 和 pane PID 只定位投递与观测；同一 tmux session 内换 pane 或 `$N` 但固定名不变时只更新投递地址。Codex thread、旧任务和旧角色不是身份选择依据。

| 当前状态 | context 的结果 |
| --- | --- |
| 当前 project 没有 master | 自动恢复或建立当前 session 的 peer，不申请批准 |
| 其他身份是 master | peer 恢复成功，并给用户一个可选的 master 替换选择；未授权时 peer 仍可使用 |
| 当前 project/session 已是 master | 复用身份与现有授权，不重复询问 |
| 用户明确授权当前身份恢复为 master | 读取当前身份与 incumbent，通过既有 authority owner 直接替换 master |

master 替换不要求调用方提供旧 token、旧 target snapshot、intent digest、lease 或 grant generation。当前事实由 daemon 读取。现有 `approve_grant` 可承载 `action=restore_master` 与用户授权原文；不要求第二份身份批准。未获 master 授权不使普通 peer 恢复失败。

## 持久化和通信

有效身份复用已有身份记录和 token，并同步恢复当前通信端点、route 与收信订阅。切换 pane 时更新所有投递事实，旧 route 自动失效。首次建立身份仍通过原 Register owner 提交真实记录。相同操作的回放复用已提交结果，不重复 master 转移。身份、master 授权、消息、任务各自由现有 owner 管理，不手工编辑 journal 或文件制造恢复结果。

已注册 session 的反查先于首次注册资料检查。反查成功时复用已提交的 adaptor；未提供新端点时不擦除原端点，也不把它写成本次观察事实。只有本次实际观察到当前 tmux session 时才执行该 session 的改名。历史 transport 中的 pane 不能触发改名或阻断无 pane 的会话恢复。同一位置缺失 host 发布时，现有绑定 writer 恢复发布，generation 保持不变；启动回放不取得该请求的授权。

服务启动逐路由隔离历史地址占用冲突。没有唯一现有持有者时不按控制槽顺序抢占；独立健康路由继续发布。控制资源损坏和持久化失败仍返回原错。新的有效恢复请求沿同一绑定 owner 收敛，不需要清除历史或额外接管函数。

身份恢复不检查工作内容、任务、claim、worktree、master 派单或历史 pane 所有权。恢复的结果必须可通信，不能仅凭角色字段宣称完成。实际持久化、端点或 I/O 错误保留原错，不伪造成功；它们不授权 agent 操作 daemon 生命周期。恢复验收通过真实发送、接收和回复完成，不增加用户必须手动执行的通信恢复步骤。

Collab 是一个全局、由用户管理的 daemon。agent 只连接现有服务。服务不可用时保留身份与任务，返回服务不可用。维护命令和凭据不属于 agent 接口，身份恢复不申请维护窗口。

## 其他宿主

非 tmux 的 AppServer（含 Codex Desktop）使用其观测到的 session/thread，DSH 使用 `DSH_SESSION_ID` 作为传输载体；两者不执行 tmux 改名，但名字仍是身份、投递和任务的锚点。任何宿主第一次绑定都分配一个固定名字；后续同 sessionID 恢复续接已有名字；新注册为新会话新增名字；旧名字在旧持有者离线或获得用户授权时可被顶替。缺失事实只按实际 adapter 所需字段补充一次。不借其他宿主的历史 pane 选择身份，不要求用户额外证明历史身份或完成第二次恢复。Desktop 或 DSH 的 control/business 提交失败不写 binding、名字或 grant，并返回原错误。

## 名字唯一锚点与一次性迁移

名字是项目内唯一的工作归属、身份、投递和任务锚点。TUI 的固定 tmux session 名字是名字意图键，Codex Desktop 的 agent session 和 DSH 的 gateway session 是承载当前执行的载体；pane、`$N`、socket、server PID、thread 和进程只定位投递与观测。

旧版本升级到新版本时，按名字做一次性迁移：旧 `NameOccupancy.pane_anchor` 只在迁移节点读取，用来把工作归到名字持有者并重建 typed 名字控制。迁移后恢复仍先读固定 tmux 名与 sessionID，二者都不命中时再用 pane 与 cwd 的共同匹配恢复旧名，也不重放旧 operation journal。新提交记录 daemon 本次观察到的 pane 与调用 cwd 联合锚点，供后续最后一级恢复使用；不把锚点镜像到日志或业务 metadata。

`rename_anchor` 是唯一改名 owner。改名成功但读回名称不一致时按改名失败处理并恢复原名；control 或 business 提交失败时回滚原名，原 binding 和 occupancy 不变。父目录 sync 返回 unknown 时保留已发布的目标名并报告 `durability_unknown`，不回滚。

## 验收边界

公开 `context` 用例覆盖：首次注册后真实 tmux session 名字变为角色注册名；已有固定名恢复时名字与 binding 不变；同 project 固定名在同一 tmux session 内换 pane、换 `$N` 或跨 cwd 保持身份；同一 pane 的不同 agent session 在 tmux 名不同时得到新名字；不同 pane 的不同 agent session 不继承旧名；固定名优先于旧 session 归属，不被旧历史否决；固定名与 sessionID 都不命中时仅 pane 与 cwd 共同匹配恢复旧名；改名失败、读回不一致及持久化失败不提交身份并保留或恢复原名；已有 master 时 peer 可用；一次用户授权直接替换 master；重复 context 不再询问或 Register；daemon PID 不变。peer 恢复与 master 恢复分别实际发送、接收并回复，检查消费回执。项目任务与旧记录保留，不成为身份恢复的拒绝条件。

外层 operation/query/cancellation 的持久化实现见 [context 合同](collab-identity-context-recovery-contract-20261008.md)。其中历史 pane/fence 设计不覆盖本文件的当前身份选择规则。图入口为 `docs/dagpipe/collab-context.graph.json` 与独立 master authority 图。
