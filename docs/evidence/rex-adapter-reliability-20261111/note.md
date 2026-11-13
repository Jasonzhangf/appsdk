# Rex 适配可靠性修复

## 长期目标

让 Collab 的 Rex 通道在真实使用路径上可靠提交、读取、报错和显示状态，并完成独立审查、合并到 `origin/main` 与资源收口。

## 本轮目标

修复四个已见问题：

1. 文本提交与 Enter 分两次执行，偶发丢 Enter。
2. `worker snapshot` 对 Rex 固定返回不支持。
3. Rex 通知拒绝错误没有 Rex 专用标签。
4. Rex `idle` 被错误显示为 working，且过期 program-status owner 可能继续投射 busy。

## 事实与证据

- Rex 版本 `0.0.1793981048-g01d6c4`，API revision `11`。
- `terminal.write` API 只把文本排队，不等待程序读取。
- 隔离探针已证明一次 `terminal.write` 发送 `text + "\r"` 可被 shell 完整执行；探针 session 已关闭。
- 官方 `capture` 是 Rex 屏幕与 scrollback 的读取入口。
- `program_status` 的 `owner` 同时包含 `pid` 和 `name`；API 明确要求两者一起比较，因为 PID 会复用。
- 当前 `notify` 先 `write_text`，再 `send_enter`；Enter 失败时文本已排队。
- 当前 `handle_worker_snapshot` 只支持 tmux，Rex 被显式拒绝为 `WORKER_SNAPSHOT_UNSUPPORTED`。
- 当前 `notification_rejected_label` 没有 Rex 分支，Rex 会落入 tmux 默认标签。
- 当前 Rex monitor 丢弃 `owner`，只把单条 `working` 映射为 busy；`idle`、`done`、`error` 和未知值都成为 unknown，现有测试甚至断言投影永不产生 idle。
- 仓库当前没有文本 `codex bridge`；用户看到的该标签不在当前 Collab 源码中，本轮只修可确认的 Rex 标签错配，不猜测其他历史层。

## 当前进度

底层 API、接入实现与设计图已核对并完成修复。产品代码修改完成，聚焦测试全绿。

- 计划：`docs/evidence/rex-adapter-reliability-20261111/plan.md`
- Ontology：`docs/design/rex-collab-ontology-20261111.md`
- DAG：`docs/dagpipe/collab-rex-adaptor.graph.json`

## 真实 Rex API 隔离验收（2026-11-11）

在新建的隔离 Rex 会话中直接复现适配器的确切调用：

- `rex capture -s S -b B --format=text --unwrap --trim` 读出屏幕与 prompt，snapshot 读取入口可用。
- 一次 `rex api call -s S com.superlogical.terminal.write` 发送 `echo REX_ONE_WRITE_OK_CR\r`，返回 `{"bytes_written":25}`；随后 capture 显示命令已执行并打印 `REX_ONE_WRITE_OK_CR`。证明正文和 CR 在单次 write 中排队后会被 shell 完整执行，不再依赖独立 Enter。
- `com.superlogical.terminal.process` 返回 foreground `{"pid":66022,"name":"zsh","cwd":...}`；`program_status` 对普通 shell 返回空 `records`，monitor 据此保持 Unknown。
- 探针会话用 `rex session kill <exact-session-id>` 关闭；未触碰其他会话或 server。

## 测试证据

- `cargo build --manifest-path collab/Cargo.toml`：通过。
- `cargo test --bin collab rex`：54 passed，0 failed。
- `cargo test --bin collab notification`：64 passed，0 failed。
- `cargo test --bin collab snapshot -- --test-threads=1`：19 passed，0 failed。
- 直跑 `cargo test --bin collab -- --test-threads=1`（CI 的门禁目标）已跑完整份 bin 单元测试并通过，随后到达 `appserver_two_tui_integration` 才失败。
- 并行跑 snapshot 相关用例会因 tmux 测试夹具 wrapper 目录互相删除而偶发失败；同组用例 `--test-threads=1` 全绿，CI 也使用 `--test-threads=1`，故不是本次回归。
- `appserver_two_tui_integration` 在 origin/main 基线（a2951311）与当前分支同样失败，错误一致（`batch notification never reached worker B native input`），属既有环境依赖，与 Rex 改动无关。
- main 远端 CI run 43034930378 已在 `client::tests::require_server_times_out_real_ping_and_keeps_io_error` 上失败（1312 passed / 1 failed），属既有偶发时序用例；该用例本地 bin 套件通过。

## 下一步

1. 剩余集成测试目标（除既有环境失败外）核对。
2. 独立 milestone review。
3. 提交、PR、合并到 `origin/main`、远端核对与 worktree 收口。

## 剩余缺口

- `codex bridge` 文案来源无法从当前源码复核；若它来自已安装旧版或 UI 历史日志，需要另外的安装/运行证据。
- 真实 Rex LLM 消费与前端状态刷新仍需在改变后的候选上重新验收。
- 主 CI 的偶发 `client` 超时断言与两 TUI 集成环境失败是既有问题，不在本次 Rex 改动范围，合并后需单独治理。

## 独立 Review 与复核（2026-11-11）

独立 reviewer（oauth / codex-auto-review，read-only，medium）返回 exit 0 的 JSON，报告两个 P1：

1. `RexProgramStatusOwner` 接受 `pid=0` 或空白 name，可能让不完整 owner 参与投影。已修复：解码端要求 `pid>0` 且 name 非空白，monitor 端把空白或零值 foreground 身份视为不可用；新增 `observe_current_drops_incomplete_owner_identity` 与 `incomplete_foreground_identity_never_projects_busy_or_waiting` 测试。
2. 缺真实前端/公开入口黑盒证据。已补：显式启用 `rex_public_context_send_recv_reply_restart_and_body_fidelity`（需 `COLLAB_REX_TEST_SERVER` 显式选择本机 Rex server），真实 Rex 会话 + 隔离 daemon 全链路通过。覆盖 working→busy、blocked→waiting_user、done→idle、通知批量投递、send/recv/reply/restart 与正文保真。

## 真实公开契约 E2E（2026-11-11）

- `COLLAB_REX_TEST_SERVER=unix:///Users/fanzhang/Library/Application%20Support/rex/server.sock cargo test --manifest-path collab/Cargo.toml --locked --test rex_public_contract -- --ignored --exact --test-threads=1`
- 结果：1 passed，0 failed。夹具自建真实 Rex receiver/sender 会话与隔离 daemon，公开 `context/notify/recv/reply/restart` 全链路执行。
- 状态链证据：`state=working` 投影 busy；`state=blocked:kind=question` 投影 waiting_user；`state=done` 投影 idle；receiver 的 program_status owner 与 Python foreground pid/name 匹配。
- 该用例第 140 行旧断言把 done 期望建模为 `unknown`；按用户要求的 idle 正确显示语义，已更新为 `idle`。

## Rex parity P0 当前修订（2026-11-12）

长期目标仍是完成 Rex parity 的真实投递修复、ontology 校验、独立 review、
合并、重建与重启。当前 P0 是先恢复同一 server/session/block 服务实例变化后的
自动投递，尤其覆盖 QQBot 默认 CEO 入站。

当前通知语义：

- Rex block 本身是 anchor。window/layer/layout 只描述布局；split 创建新 block，
  resize/move/swap 保留原 block。server/session 用于定位和核实 block。
- 通知提交先原样写入正文并验证完整 UTF-8 字节，再对同一
  server/session/block 执行一次独立 `send-key enter`。
- `REX_SERVICE_CHANGED` 只有在 `stage=BeforeWrite` 且
  `operation=server.status` 时可恢复。恢复只刷新现有服务的 instance/host
  代际，并拒绝 server、session 或 block 的任何变化。
- 刷新成功后原 durable message 只重试一次；accepted 或 unknown 结果不进入恢复
  路径，也不重写正文或重复 Enter。

已实际执行的聚焦证据：

- `server::notification_batch_tests::rex_service_generation_change_is_recovered_for_qqbot_ceo_inbound`
  1 passed，覆盖 QQBot CEO 入站、服务代际变化、同一目标刷新、一次 write、
  一次 Enter 和最终 accepted。
- `server::notification_batch_tests` 36 passed，覆盖 timer 恢复、同 block、
  wrong block 拒绝、unknown 不重试、旧 binding 替换隔离和重复 owner 拒绝。
- 公开 `channel_inbound_cli::` 用例 6 passed，覆盖原路回复、重复平台事件、
  错误 envelope、CEO 授权/替换、receiver 只投当前 CEO 和名字交接。
- `cargo fmt --manifest-path collab/Cargo.toml -- --check`、`git diff --check`
  通过。
- 9 个受影响 DAG 文件全部通过 `dagpipe graph validate`。
- `project-memory verify` 通过；Rex anchor/current-binding L3 记忆与当前设计一致。

仍待完成：

- 完整 Collab package 回归与适用 live/公开入口复验。
- 完整 milestone 的独立 review。
- commit、更新最新 `origin/main`、merge、push 和远端核对。
- 通过正式构建/安装路径重建，并由用户管理 daemon 生命周期完成后做 live
  QQBot 默认 CEO 投递、正式 recv、消费和关联回复验收。
- 本轮 `collab recv --timeout 30` 只返回 receive ID 和 timeout，没有可消费正文；
  不猜测或伪造通知内容。
