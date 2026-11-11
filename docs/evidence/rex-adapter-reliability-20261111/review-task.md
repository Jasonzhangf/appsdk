# Collab Rex 适配可靠性修复：独立只读 Review 合同

## 角色与权限

你是独立 Reviewer。只读审查，不修改产品代码、不提交、不另派 agent。宿主读取你的最终 JSON。

## 候选与 base

- 项目根：`/Volumes/Intel/playground/appsdk/rex-adapter-reliability-20261111`
- base：`a2951311`（origin/main）
- commit：null（审查工作树未提交候选，`git diff` 加未跟踪文件）
- scope.mode：`milestone`

## 用户目标

修复 Collab 的 Rex 通道四个真实缺陷，并按层次验证后合并到 main：

1. 通知发送偶发丢失 Enter，文本未提交。
2. `worker snapshot` 对 Rex 固定返回不支持。
3. Rex 通知拒绝没有 Rex 专用标签，被误记为其他传输。
4. Rex `idle` 被错误显示为 working，过期 program-status owner 仍投射 busy。

## 权威需求

亲读 `.appsdk/requirements.json`，对照适用条目（至少 `req-minimal-governance`、`req-collab-communication-notifications`、`req-collab-identity`、`req-command-logs`）作为审计标准。存在即读；为空注明跳过。

## 审查输入（先读）

- 任务笔记：`docs/evidence/rex-adapter-reliability-20261111/note.md`
- 计划：`docs/evidence/rex-adapter-reliability-20261111/plan.md`
- DAG：`docs/dagpipe/collab-rex-adaptor.graph.json`
- Ontology：`docs/design/rex-collab-ontology-20261111.md`
- 候选 diff：`git diff`（已跟踪）以及未跟踪文件 `docs/design/rex-collab-ontology-20261111.md`、`docs/evidence/rex-adapter-reliability-20261111/`

## 受影响实现（唯一 owner）

- `collab/src/adapters/rex.rs`：一次 write 排队正文 + CR；删除独立 Enter；新增官方 `capture` snapshot；program-status owner 解码。
- `collab/src/adapters/rex/tests.rs`、`collab/src/adapters/rex/fixture_rex.py`：红测与夹具。
- `collab/src/server/mod_parts/part_05.rs`：`notification_rejected_label` 增加 rex arm。
- `collab/src/server/mod_parts/part_07.rs`：`handle_worker_snapshot` 分派 Rex capture。
- `collab/src/server/rex_monitor.rs`、`rex_monitor_tests.rs`：foreground owner 匹配与 busy/idle/waiting_user 投影。
- `collab/src/server/peer_tests/part_04.rs`：Rex snapshot 公开入口测试。
- `collab/src/adapters/mod.rs`：导出新增类型。
- `docs/dagpipe/collab-rex-adaptor.graph.json`：图 v0.1.2。

## 作者验证（可核对）

- `cargo build --manifest-path collab/Cargo.toml`：通过。
- `cargo test --bin collab rex`：53 passed。
- `cargo test --bin collab notification`：64 passed。
- `cargo test --bin collab snapshot -- --test-threads=1`：19 passed。
- 完整 `--bin collab -- --test-threads=1` 单元测试通过；随后到达 `appserver_two_tui_integration` 才失败。
- 真实 Rex API 隔离探针：一次 write 携带正文 + CR 被 shell 执行；`capture` 可读；`terminal.process` 返回 foreground pid/name。
- `dagpipe graph validate`：valid，7 nodes / 6 edges。
- 已知既有环境失败：`appserver_two_tui_integration` 在 origin/main 基线同样失败；main CI run 43034930378 已在无关的 `client::tests::require_server_times_out_real_ping_and_keeps_io_error` 上失败。

## 审查重点

- 语义正确性：一次 write 是否真的包含正文 + 单个 CR；是否有重复/意外 Enter 边；unknown 是否会被误当成功。
- snapshot：是否只读、是否有界、是否伪造持久状态、失败是否会被吞成空成功。
- owner 匹配：pid 与 name 是否同时校验；过期/缺 owner 记录是否会再投射 busy；idle/done/error/未知是否映射 idle；无有效记录是否保持 unknown。
- 标签：Rex 拒绝是否产出 `REX_NOTIFICATION_REJECTED:` 且保留原错；其他 transport 标签是否不变。
- DAG/ontology：是否为当前约束下的最小充分模型，是否与实现一致，是否新增第二套治理或缓存。
- 测试：是否对应真实失败模式，是否只做测试适配而放松语义。

## 输出合同

严格输出一个 JSON 对象，禁止 Markdown、代码围栏、`VERDICT`、`PASS`、`FAIL`、`confidence` 或自行裁决。

- `contract_version` 固定为 `"1"`。
- `scope` 仅含 `mode`、`commit`、`base`，回显 `milestone`、`null`、`a2951311`。
- `module_boundary_evidence` 数组，每项含非空字符串 `module`、`owner`、`paths`、`edges`、`resources`、`gates`。
- `findings` 数组，每项含 `severity`（P0/P1/P2）、`file`、正整数 `line`、`rule`、`evidence`、`remediation`。P0/P1 必须写入；P2 记录不阻塞。
- 无发现时 `findings` 为空数组。
