# Collab 当前状态与组件

## 状态归属

生产状态根为 `$HOME/.collab`；隔离测试可指定 `COLLAB_STATE_DIR`。

```text
identity-map.json
routes.json
identities/<worker-id>/identity.json
projects/<encoded-project>/apps/<encoded-app>/.agent-collab/server/state.json
server.sock
daemon.lock
server.pid
service-record.json
log.txt
```

- `identity-map.json` 是唯一当前身份选择源。注册、换绑、换名、角色变化与恢复
  使用同一 owner。表内的 previous 不提供候选，也不否决新的有效授权。
- `routes.json` 只定位当前项目/app 存储与通信。它不能恢复或授予身份。
- `identities/` 保存表内已选定 principal 的精确凭据。不扫描旧凭据选身份。
- 每个项目/app 的 `state.json` 保存最新任务、描述、证据、消息及消费状态、
  当前关系与有效授权。变更直接更新当前文件；重启直接读取，不保存变更历史。
- `server.sock`、`daemon.lock` 和 `server.pid` 归全局用户管理的 daemon。
  不手改、不删除锁、不启动第二个 daemon。
- `service-record.json` 声明正式监督服务的根目录。安装与激活是不同动作。
- `log.txt` 只供诊断，不参与身份、任务或投递决策。

新版不读取旧存储格式。已有业务目录缺少 `state.json` 时，返回
`CURRENT_STATE_MIGRATION_REQUIRED`；不把旧项目当成新项目静默清空。
本次获授权的一次性交接由维护 owner 准备新当前文件。普通恢复不导入旧文件。

## 身份、名字和布局

cwd 选择项目。名字承载工作归属，真实 agent sessionID 承载当前执行。
同名工作者更换 sessionID 时，当前表更新绑定，并续接该名字下未关闭任务。
任务 ID、状态和证据保留。只读查询不补注册、不修表、不选择历史身份。

TUI 使用已存在的 tmux 通信端点。共享 tm-iterm manifest 拥有固定布局
`binding_id + tab_id + pane_order`。Collab 只关联既有位置，不改写 manifest。
物理名为 `<cwd末级目录>_<固定位置序号>_<名字>`；只有固定位置 1 可 promote 为 Jenny。
正常运行由 Collab 改名。tm-iterm 默认创建与解析，显式批量改名另行执行。
不能拆 session 显示名推断位置。tmux `$N`、pane、socket、PID 都不是 agent sessionID。

Desktop 直接绑定真实 sessionID/thread 和当前 bridge。DSH 直接绑定真实 gateway
sessionID。它们使用同一个身份表，不要求 tmux，也不执行 tmux 改名。
runtime profile 只在能从真实宿主观察时保存；缺少 profile 不阻碍身份恢复。

## 当前身份入口

运行 `collab context`。Desktop 可直接使用 CLI 自动发现已登记的当前 bridge，
不要求先调用 MCP。需要补充事实时，只提供返回的 `required_fields`：

```sh
collab context --provide '{"session_id":"...","thread_id":"...","endpoint":"...","namespace":"..."}'
```

不提交 worker_id、token、generation 或历史绑定来替代宿主事实。
`--query` 只投影当前表与当前状态。缺少投递事实时保留身份并报告当前缺项，
不伪造可达，也不阻止其他 scope 正常启动或恢复。

普通角色是 peer。master 由有效用户授权或现有合法委派建立，名字和布局不授予权限。
任何两个已注册对象均可普通直发；身份认证、精确收件人和回复关联继续生效。
通知接受、真正消费和回信分别报告，不互相替代。

## 维护边界

安装只写产品与 Skill，不代表运行中的 daemon 已更新。daemon 关闭、启动由用户
通过正式维护入口执行。Collab 不拥有 tmux 生命周期；任何安装、恢复或状态交接
都不关闭 tmux server、不重启 tmux，也不终止宿主 pane。

当前项目 master 可用 `collab reset --project` 清除此项目的当前数据。
它保留其他项目和全局 daemon。普通安装、身份恢复与规则升级不授权清空任务或消息。
AppSDK 的 `.appsdk/`、`.appsdk-control/` 和独立 bug 系统保持各自归属。

通过 `collab master status`、`collab who`、`collab status --all` 或
`collab worker status` 读取诊断。不要手改当前表、凭据、任务、消息或 socket
来伪造恢复成功。保留真实错误，只暂停受影响动作，继续独立工作。
