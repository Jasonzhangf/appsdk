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

## Ontology worktree handoff（2026-11-12 PT）

- 收到并消费消息 `m1794546435512-276`，subject「停止竞写 ontology 工作树并交接」，
  `collab recv` receive ID `receive-41783-1794550263587565000-0`。发送者是 unknown。
- 冻结交接对象：
  `/Volumes/Intel/playground/appsdk/collab-stable-names-20261112`，
  branch `codex/collab-stable-names-20261112`，PR157。该树属于 ontology 聊天
  `01a1cf28-1d6f-7bc1-918e-186d04f98666`，不是本 Rex 候选，我不交付、合并、推送或清理该树。
- 原通知记录我在 04:49 UTC 改写 `collab/src/server/work_assignment.rs`，
  随后暂存四处合流文件；收到停止指令后已停止写入、暂存、提交、合并、推送和清理，
  并且没有回滚任何内容。
- 交接时只读回执：`HEAD` 为 `023b8650`，`git status` 干净，branch ahead 13；
  后续提交 `48ca5fbf`（2026-11-12 22:06 PST）包含 `work_assignment.rs` 等合流文件。
  该树当前 `lsof +D` 无打开文件，无仍在写入的本树进程。
- 未验证项：该工作线尚未通过新组合 Review，我不得代为交付。
- 交接记录路径：
  `/Users/fanzhang/Documents/github/appsdk/task-runs/collab-identity-lifecycle-ontology-20261112/note.md`。
- Rex P0 继续在本人原授权工作树
  `/Volumes/Intel/playground/appsdk/rex-parity-implementation-20261110` 推进。

## Ontology 语义复核与候选收口（2026-11-13 PT）

按已批准语义复核当前未提交候选，三个产品代码改动均保持 ontology 一致：

1. `send_enter_until` 不再重复调用 `verify_service_until`。通知提交已经先完成正文
   write 与字节确认，再对同一 server/session/block 执行一次 `send-key enter`；
   Enter 内部不再另做一次服务代际核实，保留原设计的一条通知路径。
2. Enter 失败时保留 `operation: send_key` 与底层错误 `class`，不再强制改成
   `notify` / `Unknown`。这与通知提交图一致：正文阶段和 Enter 阶段分别保留实际
   `known-not-delivered` / `unknown` 及真实操作字段，不把 Enter 失败伪装成通知入口失败。
3. `handle_master_status` 增加 `master_wake`，使用现有
   `project_master_wake_idle_capacity` 投影，与 board 和 context 的 master 唤醒容量
   保持同一事实来源。

聚焦测试证据：

- `cargo test --manifest-path collab/Cargo.toml --bin collab rex -- --test-threads=1`
  80 passed，0 failed。
- `cargo test --manifest-path collab/Cargo.toml --bin collab master_wake -- --test-threads=1`
  16 passed，0 failed。
- `cargo fmt --manifest-path collab/Cargo.toml -- --check` 与 `git diff --check` 通过。

下一步：

- 提交当前候选，合入最新 `origin/main`（当前 `ae8ff52b`）并解决冲突，保留 main
  的身份、名字和任务语义。
- 合流后重跑 Rex、`master_wake`、`notification_batch_tests`、`channel_inbound_cli`
  公开用例，并跑完整 Collab bin 套件（`--test-threads=1`）。
- 校验受影响 DAG 与 `project-memory verify`，再跑公开 Rex E2E。
- 完成一次独立 milestone review，推 PR，等 CI，再 `gh pr merge --admin`。

## 合流最新 main 后验证（2026-11-13 PT）

- 候选提交 `3299da4d`；合入 `origin/main` `ae8ff52b` 得到 `91dec687`。
- 唯一冲突为 `collab-notification-consumption.graph.json` 描述：保留本候选 0.3.0 的
  精确收件人 / Rex block owner 语义，同时并入 main 的非阻塞 recv 与空结果不生成消费回执。
- 完整 Collab bin（`--test-threads=1`）：1368 passed，0 failed，1 ignored。首次全量运行有 15 个失败：
  两个 Rex 恢复用例因真实 CLI 进程退出超时，随后 `TEST_ENV_LOCK` PoisonError 连锁；
  单独复跑全部通过，第二次全量运行全绿。记为既有测试夹具时序不稳定，不是本候选回归。
- 聚焦组：`rex` 80、`master_wake` 16、`notification_batch_tests` 36、公开 `channel_inbound_cli` 6，全部通过。
- 公开 Rex E2E（真实 Rex server，`--ignored`）：4 passed；测试前后 Rex session 列表相同，无残留。
- 5 个受影响 DAG `dagpipe graph validate` 通过；`project-memory verify` 通过。
- 独立 milestone review 合同：`milestone-review-20261113-task.md`。

## 精确缺失退役与伪造服务标记验证（2026-11-13 PT）

本轮补上上一节未提交的两个边界，进一步收紧 ontology 的 block anchor 语义：

1. `resolve_target` 只在官方 `session.list`/`session.list_blocks` 返回精确
   `session_not_found`/`block_not_found`，或 `resolve_target` 的精确 absence
   结论出现时，才把 stale-identity maintenance 判为 Missing 并退役执行绑定。
   `server.status` 的 generation/timeout、缺 `sessions`/`blocks` 数组、以及未知
   原生错误都保留 Unknown，不触发名字绑定退役。
2. `server.status` 的 service-change 错误增加 typed generation-mismatch 标记；
   `is_refreshable_service_change` 不再只凭 `REX_SERVICE_CHANGED` 文本判定，
   而要求 `detail=` 后携带该 typed marker，防止伪造或不完整 native detail 被当作
   可恢复代际变化。Enter 失败仍保留真实 `send_key` 操作与底层错误 class。

验证：

- `cargo test --manifest-path collab/Cargo.toml --locked --bin collab rex -- --test-threads=1`
  87 passed，0 failed。
- 完整 `cargo test --manifest-path collab/Cargo.toml --locked --bin collab -- --test-threads=1`
  1375 passed，0 failed，1 ignored。
- 真实 Rex 公开契约
  `COLLAB_REX_TEST_SERVER=unix:///Users/fanzhang/Library/Application%20Support/rex/server.sock cargo test --manifest-path collab/Cargo.toml --locked --test rex_public_contract -- --ignored --test-threads=1`
  4 passed，0 failed。
- `cargo fmt --manifest-path collab/Cargo.toml -- --check`、`git diff --check` 通过。
- 正式构建入口 `scripts/build-collab.sh` 通过，产物版本
  `collab_build_version=0.2.0495`。直接 `cargo build --release` 被项目
  `build.rs` 拒绝，未绕过门禁。
- 10 个受影响 DAG 全部通过 `dagpipe graph validate`。
- `project-memory verify` 通过；`project-memory query "rex anchor block session window"`
  返回三条一致记录，均确认 block 是 anchor、split 创建新 block、布局变化不改 block、
  仅权威 block 缺失退役名字绑定，没有冲突结论。

## 最终候选回归与交付前状态（2026-11-13 PT）

- 完整 Collab bin 首次重跑出现 1 个真实 tmux fixture failure：默认
  `IsolatedTmux::start()` 未显式启动 shell，ready probe 在 tmux 启动进程退出后未被
  执行；其余 27 个 `PoisonError` 是 `TEST_ENV_LOCK` 连锁失败。
- 修复只作用于测试夹具：默认隔离 tmux session 显式启动 `/bin/sh`，并保留唯一 marker
  probe 作为 shell 可执行性确认。受影响 tmux 组随后通过 `38 passed, 0 failed, 1 ignored`。
- 修复后的完整命令
  `cargo test --manifest-path collab/Cargo.toml --locked --bin collab -- --test-threads=1`
  通过：`1491 passed, 0 failed, 2 ignored`，耗时 421.02 秒。
- 该失败没有改变 Rex ontology 或生产路径；`TEST_ENV_LOCK` 连锁不再出现。当前候选仍需
  完成基于最新 `origin/main=56548a85` 的独立 milestone review、提交、push、PR CI、
  main 合并和正式 `scripts/build-collab.sh` 构建。

## 提交前最终候选（2026-11-13 PT）

- 最终夹具与 Rex activity 修复已提交为 `87acb6d4`：
  `fix(collab): complete Rex activity and fixture readiness`。
- `origin/main=56548a85` 已是当前提交的祖先；未发生新的冲突，候选相对
  `origin/main` 保留 Rex block-anchor、同目标代际刷新、精确缺失退役和主线身份/任务语义。
- 提交后聚焦回归：
  `cargo test --manifest-path collab/Cargo.toml --locked --bin collab rex -- --test-threads=1`
  为 `117 passed, 0 failed`；`master_wake`、`notification_batch_tests` 和
  `channel_inbound_cli` 继续通过。
- 旧 review 输出不适用于本候选。必须重新执行合同
  `milestone-review-20261113-task.md`，其当前候选为 `87acb6d4`，再进入远端交付。

## 最终审查前 P2 收口（2026-11-13 PT）

- 独立 reviewer 针对 `87acb6d4` 未发现 P0/P1，提出 1 个 P2：恢复谓词此前只解析格式化错误字符串，原生 `server.status` 错误若伪造同名 detail 可能获得错误的恢复资格。
- 已在 `e4430cf2` 修复：恢复条件现在还要求 `native_code=none`；新增原生错误伪造 marker 的拒绝用例和本地 typed generation-mismatch 的放行用例。
- 修复后 Rex 聚焦组单线程通过 `119 passed, 0 failed`，格式和 diff 检查通过。
- 当前必须基于包含审查范围记录的最终提交 `679d9a08` 重新执行独立 milestone review；无 P0/P1 后再 push、等 CI、合并和正式构建。
