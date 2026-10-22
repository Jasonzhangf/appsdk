# 稳定名字、会话与工作继承

状态：目标合同。安装 Skill 不等于 daemon 已加载实现；运行行为须单独验收。

## 恢复操作：context 自己迁移，agent 不做历史清理

普通注册或恢复只调用：

```sh
collab context
```

daemon 负责旧身份兼容、重复 session 收敛、旧 route 退役、当前名字/session/通信
换绑及有效订阅对齐。agent 不枚举档案选身份，不比较时间戳、token 或 generation，
不先 close 历史 worker，不编辑或删除 identity/route/journal，不 reset 项目，也不
为迁移启动或重启服务。旧记录不拥有否决当前恢复的权力。

内部选择顺序固定，不由 agent 判断候选：

| 当前输入或状态 | context 的责任 |
| --- | --- |
| 用户已明确授权精确恢复目标 | 先处理该意图，再做旧状态兼容；历史歧义不能提前否决 |
| 当前 session 有唯一有效 typed 名字归属 | 续接该名字及当前工作，刷新执行和通信绑定 |
| 同 session 有多个旧 binding 或仅有重复历史档案 | 在服务端收敛；无唯一当前归属时从当前 session 建立稳定载体，不按档案顺序、mtime 或 pane 猜 |
| 同项目/app/session 新注册成功提交 | 新载体取代旧投递绑定；保留名字工作和历史，不停止旧进程 |
| 不同 session 在线持有用户指定名字 | 没有本次确认时返回明确确认请求；已有确认直接应用，不再次询问 |
| 名字占用确实 unknown | 沿现有持久裁决；用户明确恢复并确认接管不可用 Jenny 时直接接管 |
| 确实缺当前事实、服务不可达或持久写入失败 | 返回具体缺项或原错，区分未完成与成功；不转成历史身份选择题 |

用户明确要求恢复普通 peer 时，用现有控制入口表达该意图，不猜旧 worker ID：

```sh
collab context --approve-grant '{"decision":"approved","action":"restore_peer","authorization":"<本次用户原话>"}'
```

用户明确指定既有 worker 身份时，目标来自本次用户指令或 daemon 返回的精确目标：

```sh
collab context --approve-identity '{"decision":"approved","action":"restore_peer","target_identity":"<明确目标 worker ID>","authorization":"<本次用户原话>"}'
```

`restore_identity` 保持目标角色；`restore_peer` 明确恢复为 peer。恢复 Jenny/master
使用既有 `--approve-grant` 的 `restore_master` 意图，不能从普通 peer 恢复推导 master。
用户指定普通名字时，用 `restore_name` 和 `requested_name`，不用 display label 代替。
不同 session 在线占名的接管用 `replace_confirmed:true` 表达已取得的本次确认；
同 session 正常恢复不要求这个确认，不重复索取已提供的授权。

daemon 自行读取 scope 和当前占用引用。agent 不构造 `expected_incumbent`、digest、
时间戳或历史 fence。已有完整 approval 仍兼容。需要保留操作号时使用结果中的
operation_id；只读查进度用 `collab context --query --op '<operation_id>'`，query 不带
approval。曾失败的操作保留失败记录；发起新的用户恢复意图时开启新 invocation，
不以同一失败操作的幂等回执冒充再次执行。

正常结果为 `completed` 且 snapshot `registered=true`，同时报告实际名字、角色、
session 和当前 binding；queued/partial/pending 不等于恢复完成。确实缺运行事实时，
只补结果 `requires`/`required_fields` 指定的真实字段，不探测别人的 pane 来猜。
历史档案歧义属于 context 内部迁移事项，不能作为 agent 的工作分支。若旧已加载
runtime 仍返回裸 `IDENTITY_RESTORE_AMBIGUOUS`，保留 operation_id 和原错，报告实现
或版本缺陷；不要把错误当成用户授权不足，更不要手工清理以绕过。未受影响工作继续。

## 名字、角色与会话

名字承载项目内工作归属，master/peer 表示角色和权限，agent sessionID 是当前
执行载体。注册报告 agent sessionID 并由 daemon 绑定。tmux session、pane、socket
和 server PID 仅定位探测与投递，不是身份或任务键；明确弃用旧的 tmux session
选择 peer 规则，不以历史 pane/native 绑定否决当前恢复。

名字仍归当前工作者而 sessionID 变化时，daemon 更新名字的 sessionID，并自动
同步 board 中该名字下全部未关闭任务的 sessionID。任务 ID、状态、证据、资源与
历史保持，已关闭任务不回写。当前名字工作者直接续接旧任务，不要求 Jenny 再次
assign，不以旧 session 不一致阻塞。旧执行者不另行通知、停止、撤权或管控。

名字交接也转交该名字的未消费消息，更新当前收件与唤醒目标。消息 ID、正文、原
发送者和真实消费回执保持；已消费历史不回写、不重放。队列接受、adaptor 接受与
目标消费分别报告，不能因换绑把尚未消费消息标为已消费。

## 分配、恢复与裁决

daemon 按项目管理名字。master 为 Jenny，普通名字按固定顺序取首个可用项：

`Alice, Bob, Carol, David, Emma, Frank, Grace, Henry, Iris, Jack, Kate, Leo, Mia, Noah, Olivia, Peter, Quinn, Rose, Sam, Tina`

Jenny 不占普通池名额，各项目独立占用。池满返回明确错误。普通恢复按 agent
sessionID 找当前名字；同名正常续接更新当前 session 与位置，不重复确认。用户
指定恢复 Jenny 时同时接管 master 及该名字工作。其他工作者在线占名时默认不
替换，用户要求接管需本次二次确认。确认离线后失去保持权，可重新分配。
Jenny 自身 unknown 且无法响应时，用户明确指定恢复 Jenny 并完成本次接管确认，
直接接管 Jenny 名字和 master。用户确认高于 Jenny 裁决，不等待旧执行者。

原 session 重连，原名未被接管则自动恢复；已被接管则从池分配新名，不自动抢回。
master 已被明确清除或自身注销时，普通 `context` 恢复 peer，不以历史 Jenny 占用
要求授权或自动恢复旧权限。只有仍有效的 master grant 或本次指定 Jenny 的授权
恢复才能取得 Jenny/master。
新持有者继承该名字未完成工作、总结、笔记和经验，不创建空白替身。

daemon 周期观察当前具体 agent 是否在场及其 activity。Codex 当前在场为 online；
busy、idle、waiting_user 均保名，waiting_user 不可当可派单 idle。当前轮明确等待
后台命令属于 busy；后台事实不足以分类时保留 activity unknown。确认回 shell 且
无 agent 为 offline；历史滚屏不算在场。探测失败或不支持为 unknown，不据此释放。

在线归属 unknown 时由当前项目 Jenny 获取当前 pane snapshot 裁决，结果交 daemon
应用。裁决应用后，归属名字的当前工作者继续任务并同步 session 与未关闭任务。
仍 unknown、snapshot 失败或 Jenny 不可用时保留原绑定和待决请求，不猜测离线。
online/activity unknown 已确认在线，不触发归属裁决。检测由 daemon 统一调度。

## 任务与角色交接

名字当前无可用工作者时，未关闭任务显示空，持久名字引用与任务保留。向当前
Jenny 说明因原绑定不可用回流，需要她重新 assign；不自动让 Jenny 执行。未改派
时名字恢复自动接续，已改派任务跟随新名字。

master 和 peer 都主要承接长程目标，负责拆分、派发、资源管理、验收与交付，优先
让子 agent 编码。一次性执行/观察发独立 `codex exec --profile gcm`；长程目标交
可用且获授权 peer。无 peer 时保留长程目标并拆分。注册和恢复说明双方职责，建议
按各自关注的目标、P0/P1、阻塞和资源设置有限的定时巡检；peer 阻塞先问 master。
peer 对自己的日常长程任务主动建立并维护巡检任务与订阅。每次收到 master 新任务，
先评估目标、依赖、阻塞、优先级及资源，再更新已有巡检任务和订阅；仅补缺项、
移除已过时的自有范围，避免重复定时器。回报巡检范围变化或无需变更的具体原因。
任务完成后由 agent 主动关闭该任务的巡检任务和定时订阅。没有新任务、无事可做时，
立即关闭巡检，不续订或重建空循环。接收新任务的消息订阅继续保留；新任务到来后，
重新评估再开启所需巡检。只关闭自己的巡检范围，不取消其他目标的订阅。

恢复名字后读取项目 orchestration-local 中该名字的总结与派发记录，从未完成节点
继续。升级交接报告名字、角色、当前 agent sessionID、未完成目标、资源及巡检项。
不重启 tmux，不重建 pane，不手改 journal，也不由 agent 启停全局 daemon。
