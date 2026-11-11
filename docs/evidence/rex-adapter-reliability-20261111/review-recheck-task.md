# Collab Rex 适配修复：finding 复核合同

## 角色与权限

你是独立 Reviewer。只读，不修改产品代码、不提交、不另派 agent。宿主读取你的最终 JSON。

## 候选与 base

- 项目根：`/Volumes/Intel/playground/appsdk/rex-adapter-reliability-20261111`
- base：`a2951311`（origin/main）
- commit：null（审查工作树未提交候选，`git diff` 加未跟踪文件）
- scope.mode：`milestone`

## 复核目标

上一轮同入口 reviewer 报告两个 P1，现已修复。只复核这两处修订及其受影响面，复用上一轮仍有效的结论，不重开全量审查。

### Finding 1（已报 P1）：owner 完整性

- 原问题：`RexProgramStatusOwner` 接受 `pid=0` 或空白 name；monitor 直接比较，可能让不完整 owner 进入当前记录集并投射 busy/waiting_user。
- 修复：`collab/src/adapters/rex.rs` 解码端要求 `pid>0` 且 name 非空白，否则丢弃 owner；`collab/src/server/rex_monitor.rs` 把空白或 `pid<=0` 的 foreground 身份视为不可用并保持 unknown。
- 新增测试：`observe_current_drops_incomplete_owner_identity`（rex 适配测试）、`incomplete_foreground_identity_never_projects_busy_or_waiting`（monitor 测试）。

### Finding 2（已报 P1）：缺真实前端/公开入口黑盒证据

- 新增：显式启用 `collab/tests/rex_public_contract.rs::rex_public_context_send_recv_reply_restart_and_body_fidelity`（需 `COLLAB_REX_TEST_SERVER` 显式选择本机 Rex server）；真实 Rex 会话 + 隔离 daemon 通过。
- 覆盖：working→busy、blocked→waiting_user、done→idle、通知批量投递、send/recv/reply/restart、正文保真。
- 同时把该用例第 140 行旧断言 `unknown` 更新为新语义 `idle`（用户要求 idle 正确显示）。

## 审查输入

- 任务笔记：`docs/evidence/rex-adapter-reliability-20261111/note.md`
- 计划：`docs/evidence/rex-adapter-reliability-20261111/plan.md`
- Ontology：`docs/design/rex-collab-ontology-20261111.md`
- 候选 diff：`git diff`（含 `collab/src/adapters/rex.rs`、`collab/src/server/rex_monitor.rs`、`collab/src/adapters/rex/tests.rs`、`collab/src/server/rex_monitor_tests.rs`、`collab/tests/rex_public_contract.rs`）

## 复核重点

- owner 完整性校验是否真实生效、是否有绕过路径；foreground 不完整是否确实保持 unknown。
- E2E 是否是真实公开入口、是否真的覆盖四个受影响行为，是否只改测试适配而放松语义。

## 输出合同

严格输出一个 JSON 对象，禁止 Markdown、代码围栏、`VERDICT`、`PASS`、`FAIL`、`confidence` 或自行裁决。

- `contract_version` 固定为 `"1"`。
- `scope` 仅含 `mode`、`commit`、`base`，回显 `milestone`、`null`、`a2951311`。
- `module_boundary_evidence` 数组，每项含非空字符串 `module`、`owner`、`paths`、`edges`、`resources`、`gates`。
- `findings` 数组，每项含 `severity`（P0/P1/P2）、`file`、正整数 `line`、`rule`、`evidence`、`remediation`。
- 无发现时 `findings` 为空数组。
