# Collab 与 tm-iterm 共用布局接口

状态：2026-11-05 实现合同。Collab writer 为 appsdk.Jack；tm-iterm writer 为
server.Jenny。两者分别修改自己的源码。候选源码、安装与live验收分别记录。

## 唯一归属

| 资源 | 唯一 owner | 对方用途 |
|---|---|---|
| tm-iterm manifest | tm-iterm | Collab 只读既有 binding，不重排或改写 |
| identity-map.json | Collab BindingControlStore | tm-iterm 只读 active current，不写身份 |
| master grant | Collab grant owner | 布局和名字均不授予 master |
| 正常注册物理改名 | Collab | tm-iterm 默认只创建缺失 session |
| 显式初始化批量改名 | tm-iterm 的显式脚本 | 用户本次允许使用；默认命令不得触发 |

Jason 最新要求：只有固定布局 1 号可以 promote 为 Jenny。其他位置的 peer
不能 promote，也不能为角色变化移动。位置跟布局 binding，不跟名字池索引。

## 真实格式

manifest 继续使用已有 JSON array。其 `binding_id / tab_id / pane_order / profile`
保持。旧 `name / tmux_session / codex_session_id` 不能覆盖 Collab current。

Collab 文件的默认路径是 `~/.collab/identity-map.json`，格式如下。
这是不含凭据的结构示例，不是生产名册。

```json
{
  "schema_version": 1,
  "slots": [{
    "identity_key": {
      "project_scope": "/projects/routecodex",
      "app_scope_id": "appserver-cli",
      "name": "Alice"
    },
    "identity_status": "active",
    "display_slot": 2,
    "generation": 1,
    "current": {
      "layout_binding_id": "tm_fixed_five",
      "binding": {
        "project_scope": "/projects/routecodex",
        "app_scope_id": "appserver-cli",
        "agent_id": "current-worker",
        "runtime_id": "current-runtime",
        "binding_id": "collab-binding",
        "session_id": "real-agent-session",
        "native_thread_id": "real-thread",
        "tmux_endpoint": {
          "socket_path": "/tmp/tmux.sock",
          "server_pid": 100,
          "tmux_session_id": "$4",
          "pane_id": "%4",
          "pane_pid": 101
        }
      },
      "transport": {"kind": "tmux", "namespace": "$4"},
      "tab_id": {"project_scope": "/projects/routecodex", "app_scope_id": "appserver-cli"},
      "host": {"agent_kind": "codex", "host_type": "codex_tui", "profile": "gcm"},
      "invocation_cwd": "/projects/routecodex",
      "physical_name": "routecodex_5_Alice"
    },
    "previous": null
  }],
  "effects": []
}
```

`generation` 只是 store 的 CAS，不用于身份选择、route 排序或 tm 显示位置。
`display_slot` 是名字池展示索引，不是 manifest pane_order。
`current.tab_id` 是 Collab 逻辑 scope，不是 iTerm tab ID。
endpoint_generation 已退役。消费者不得要求该键，也不得用旧值选择 current。

## 读取和连接

1. 只接受已支持的 schema_version。坏 JSON 或未知版本返回原错。
2. 只读取 identity_status=active 且 current 非空的 slots。
3. 按 current.layout_binding_id 连接 manifest.binding_id。
4. 名字取 slot.identity_key.name。agent sessionID 取 current.binding.session_id。
5. agent/profile 取 current.host.agent_kind、host_type、profile。
6. live tmux 地址取 current.binding.tmux_endpoint。物理标签取 current.physical_name，
   使用时核实同一 typed tmux session 的实时 readback；不解析标签恢复身份或布局。

profile 是 runtime profile，不是 manifest.profile 的 iTerm profile。
Desktop/DSH 没有 layout_binding_id，tm 消费时不将它们塞入 pane。
previous、effects、旧 manifest sessionID 均不提供恢复候选。
map 没有 grant 投影，tm 不从 Jenny 名字推断真实 master 权限。

Collab 首次关联布局只用已验证 endpoint 的完整实时 session 名与 manifest
tmux_session 精确匹配，不拆名字。关联后只保存 layout_binding_id，后续按 ID
读取固定布局。manifest 过时、缺失或损坏不改变已选身份；layout 状态单独呈现。

## 防冲突与验收

- 正常 tm create/restore 在创建前按链接的 map current 解析当前目标，不能按旧
  manifest 名再建一份 session。创建和布局恢复不改变身份。
- 正常 Collab context 不调用 tm create、restore、adopt 或 batch names。
- 显式初始化 names 脚本不得被默认命令隐式调用；不得写身份表或重排固定布局。
- resume-plan 保持只读。恢复命令只使用 map 当前 agent sessionID 和适用 profile。
- AppSDK 公开 fixture 已覆盖：固定5号注册仍在5号；promote拒绝且不改表/物理名；
  固定1号可promote，manifest全部字节保持；缺表不继承历史任务；坏表显式报错。
- tm owner 需补当前格式解析、稳定ID连接、旧manifest名字不生成重复session、
  resume-plan无旧session回退，以及默认命令不触发rename的正反证据。

本合同不授权生产 manifest 初始化、安装、daemon 生命周期或 tmux server 生命周期。
