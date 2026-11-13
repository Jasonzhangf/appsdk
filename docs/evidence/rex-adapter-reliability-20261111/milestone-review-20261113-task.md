# Rex parity milestone：整体独立审查合同（2026-11-13）

## 角色与权限

你是独立 Reviewer。只读。不修改文件，不提交，不另派 agent，不运行会改变 Rex、tmux、daemon 或全局安装状态的命令。宿主读取你的最终 JSON。

## 候选与 base

- 项目根：`/Volumes/Intel/playground/appsdk/rex-parity-implementation-20261110`
- base：`origin/main`（`56548a85`）
- commit：`e4430cf2`（审查该提交相对 `origin/main` 的候选差异）
- scope.mode：`milestone`

## 审查目标

审查 Rex parity milestone 的完整功能和整体架构，是否与用户已批准的 ontology 一致，并且是否保留最新 main 的身份、名字和任务语义。

已批准语义：

1. Rex block 本身是 anchor。window/layout 只描述布局。split 创建新 block；resize/move/swap 保持同一 block。server/session 只定位和核实 block。
2. 有效 current binding 到 block 为 1..1；block 到 current owner 为 0..1。后来的成功绑定替换旧 owner；旧身份、任务、消息和资源责任保留。
3. 权威事实确认 block 不存在时退役绑定的名字；unknown 或查询失败不算不存在。
4. 通知先原样写入 UTF-8 正文并确认完整字节，再对同一 server/session/block 发一次独立 `send-key enter`。正文失败或短写不按 Enter；Enter 失败不重写正文也不再按。Enter 明确拒绝为 KnownNotDelivered，未知为 Unknown，阶段都保持 WriteQueued。
5. `REX_SERVICE_CHANGED` 只在 `stage=BeforeWrite` 且 `operation=server.status` 时可恢复：刷新同一 block 的服务代际并只重试一次原 durable message。
6. 活动投影：owner pid 与 name 必须同时匹配 foreground。有效 blocked -> waiting_user，working -> busy，idle/done/error/未知 state -> idle，无有效记录 -> unknown。

## 审查输入

- 需求：`.appsdk/requirements.json`（存在时亲读适用条目）
- 任务笔记：`docs/evidence/rex-adapter-reliability-20261111/note.md`
- 计划：`docs/evidence/rex-adapter-reliability-20261111/plan.md`
- 整体模型：`docs/design/rex-project-agent-lifecycle-architect-20261110.md`
- Adapter ontology：`docs/design/rex-collab-ontology-20261111.md`
- DAG：`docs/dagpipe/collab-rex-adaptor.graph.json` 和 `docs/dagpipe/collab-notification-consumption.graph.json`
- 候选 diff：`git diff origin/main...e4430cf2`，重点为 `collab/src/adapters/rex.rs`、`collab/src/server/rex_monitor.rs`、`collab/src/server/notification_batch_tests.rs`、`collab/src/server/mod_parts/part_08.rs`、`collab/src/server/binding_slot.rs`、`collab/src/adapters/tmux.rs`、`collab/tests/rex_public_contract.rs`

## 作者已完成验证

- `cargo test --manifest-path collab/Cargo.toml --locked --bin collab -- --test-threads=1`：1491 passed，0 failed，2 ignored。此前一次全量运行中的 1 个 tmux fixture failure 已定位为默认隔离 shell 未显式启动，随后触发的 27 个 `TEST_ENV_LOCK` PoisonError 为连锁；修复夹具后完整套件通过。最终 fixture 修复已包含在提交 `87acb6d4`，显式启动 `/bin/sh`，不改变生产路径；最终候选另含 `e4430cf2` 的 typed recovery marker 修复。
- `rex`、`master_wake`、`notification_batch_tests` 聚焦组全部通过。
- `appserver_two_tui_integration channel_inbound_cli`：6 passed。
- `COLLAB_REX_TEST_SERVER=... cargo test --test rex_public_contract -- --ignored`：4 passed，真实 Rex 会话，无残留 session。
- 10 个受影响 DAG `dagpipe graph validate` 通过，`project-memory verify` 通过，`cargo fmt --check` 通过。

## 审查重点

- 代码与上述语义是否一致，是否有绕过、双路径、fallback、吞错或伪成功。
- block 替换是否只改 current owner，不注销身份、不迁移业务、不选择其他 block。
- 合流是否丢失 main 的非阻塞 recv、reviewer 执行准入、cleanup 内容证明和 worker ID 规范化。
- 黑盒用例是否真实覆盖成功、失败和副作用，是否有放松断言。

## 输出合同

严格输出一个 JSON 对象，禁止 Markdown、代码围栏、`VERDICT`、`PASS`、`FAIL`、`confidence` 或自行裁决。

- `contract_version` 固定为 `"1"`。
- `scope` 仅含 `mode`、`commit`、`base`，回显 `milestone`、`e4430cf2`、`56548a85`。
- `module_boundary_evidence` 数组，每项含非空字符串 `module`、`owner`、`paths`、`edges`、`resources`、`gates`。
- `findings` 数组，每项含 `severity`（P0/P1/P2）、`file`、正整数 `line`、`rule`、`evidence`、`remediation`。
- 无发现时 `findings` 为空数组。
