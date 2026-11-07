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

AppServer 首次注册或真实换端点时可补本次事实：

```sh
collab context --provide '{"session_id":"<当前 agent sessionID>","thread_id":"<当前 native thread ID>","endpoint":"<当前 AppServer endpoint>","namespace":"codex_tui"}'
```

不能把 agent sessionID、native thread ID 和 tmux `$N` 互相替代。
已经命中当前映射时可复用该映射的 adaptor；复用不表示观测到了新的端点。
不要从旧 transcript 抄信息，不猜 socket，也不把 MCP tools pipe 当 AppServer。

`collab context --query` 只读当前映射和当前快照，不查旧操作，也不需要操作能力码。
`--op` 仅关联本次调用，不建立操作历史或恢复 selector。

## 当前授权与角色

只有用户授权才恢复 master。普通 peer 恢复不推导 master 权限。

```sh
collab context --approve-grant '{"action":"restore_master","authorization":"<本次用户授权>"}'
collab context --approve-grant '{"action":"restore_peer","authorization":"<本次用户授权>"}'
collab context --approve-grant '{"action":"restore_name","requested_name":"Alice","authorization":"<本次用户授权>"}'
```

授权作用于本次 map 选定的当前对象。不要提交旧 `target_identity`、incumbent fence、
digest、generation、时间戳、裁决请求或旧完整 approval。旧身份准入和 grant
replacement 阶段链已删除。当前 scope 不符或授权缺失会明确失败。

正常成功结果是 `completed`，且 snapshot 的 `registered=true`。读取其中的实际
名字、角色和当前绑定继续工作。pending、queued、missing_facts 和错误都不是恢复
完成。只补返回结果要求的真实事实，不让 agent 选择历史身份。

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

Desktop 和 DSH 使用同一身份表，只是绑定方式不同。Desktop 使用宿主 bridge 的
当前 sessionID 和真实 caller 能力；DSH 使用 gateway 的当前 sessionID、endpoint
和接收循环。两者不依赖 tmux 改名，也不增加不存在的 native 字段。

名字续接同步该名字下未关闭任务的当前 sessionID，保留任务 ID、证据、资源与状态。
名字交接转交未消费消息，并对齐当前收件和唤醒目标。已消费消息不重放，不伪造
消费结果。队列接受、宿主接受和真正消费分别验收。

任何身份、升级或通信恢复都不关闭、重启 tmux server，也不终止宿主 pane。
