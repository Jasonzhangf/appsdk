# 本目标节点笔记

## 2026-10-09 — tmux 纠正与实施计划审阅

- tmux 隔离 daemon 公共 CLI/MCP 测试为 **4/4 通过**，是有效测试证据，不是阻塞项；原始 Goal 与 T2 回执已一致记录。
- 当前 Goal 仍为 9 个节点：3 个完成、2 个进行中、4 个未开始。D2/D3 设计准入通过；产品实现未开始。
- 已审阅 fresh planner 提案 `planner-implementation-20261009/plan.md`，接受其实施依赖顺序并记录 parent refinement：B23-O 必须有 host-only degraded query path，才能在 project replay 失败后仍提供只读 outer-operation query。`HostPaths::journal_path()` 与 project journal 是不同 owner；当前启动顺序会在 bind listener 前因 project replay error 退出。
- K23 源码证据：`implementation-k23/source-observation.md`；计划裁定：`implementation-k23/plan-assessment.md`；冻结合同：`implementation-k23/b23-implementation-contract.md`。B23-O 是下一可执行增量，范围包括 host-only degraded query，不包括 D2/D3 上层行为。
- 由于当前 Desktop 执行者未注册 Collab，read-only board 查询返回 `BOARD_IDENTITY_REQUIRED`；未调用 bootstrap、未注册 peer、未 promote master。按 Goal 改用新建独立 fresh gcm worker，session `69634`、thread `01a11ff7-9d5f-7011-8335-a7487fa627b3`，负责 B23-O；记录与原始 events 位于 `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/implementation-b23-operation-journal/`。
- AppSDK development intake 已在候选工件前完成：issue `86b6e8b`，分类 `feature`，dedup query 无先前记录、exit 0。此 issue 绑定 Goal/worktree/实现/测试/后续 review/merge。
- B23-O 当前状态：session `69634` 仍 live；作者已新增 `collab/src/server/operation_journal.rs`，并开始改 `collab/src/proto.rs`、`collab/src/server/mod.rs`、`collab/src/server/mod_parts/part_12.rs`。设计方向是复用 D2-B 的 `IdentityContext` query envelope，并保持 project replay error 时 host query-only listener。还没有测试或 compile 结果，不能判子节点完成；候选中其它 dirty 路径保持原样。
- 未运行测试、未触碰 daemon/身份、未改产品源码、未安装/重启、未做 Git 集成。A6/Create 继续 BLOCKED。

## T0 — 最新观察完成

- 任务：collab-context-identity-peer-crud-remediation-20261008；controller：本 Goal。
- 输入：最新 origin/main 3dfdaf8503b7a6f6a76651a1e282c038b6648c3a；独占 worktree 与分支见任务真源。
- 结果：确认 F1 安装公开入口空成功；PR17 已修 Missing 关闭边界；Desktop socket 现在可连接，不能复用旧拒连结论。
- 证据：独占 run/observation.md；历史 audit/public-entry-observations.json 只做历史参考。
- 资源：本任务 worktree、独占 .worker-runs/collab-context-identity-peer-crud-20261008、planner/home。既有 daemon 35786 与其他 worktree 非本任务资源，不清理。
- 文档：仅把本任务 Goal 和两份审计文件原内容复制进候选；未复制他人 docs/collab.md dirty 改动。

## T1 — 独立规划已启动

- fresh codex exec --profile oauth --model gpt-6.1-sol --sandbox read-only --ephemeral；session 50630，codex worker PID 25733（以活进程重新核验为准）。
- 输入：run/observation.md 与 planner/plan-task.md；输出：planner/plan.md、events.jsonl、stderr.log。
- 状态：运行中，尚无 accepted plan。MCP 初始化有本地 endpoint 连接警告，但 worker 仍活；不是终态或 PASS。
- 下一步：读回真实 planner 结果，评估方向与验收，补必要能力观测并完成设计准入；不启动未经规划的产品实现。

## T0 补证 — 真实 Native API / 安装公开入口

- 正式稳定 codex app-server 独占实例：thread/start/read/archive 成功，实际 metadata 含 sessionId/cwd；未用 mock。
- 最新独占 consumer：两次 canonical installed collab context 均 registered=true，worker_id 相同。归档线程、isolated collab down 和 owned AppServer TERM/exit 完成。
- 证据：native-api-receipt.json、native-public-receipt.json、native-capability.md。此为普通 Native 登记/重放证据，不是批准身份恢复或 peer CRUD 的证据。
- 首次错误（不合法 --profile）与 playground 祖先 consumer 拒绝已保留在独占 run；只修探测参数/fixture 位置，未放宽产品 admission。
- 临时短 socket 目录和能力 probe home 仍属本任务，待证据接收后回收；没有操作生产 identity/master。

## T1 接收与 O1/O2 派发

- 独立 planner session 50630 已退出 0，结果 BLOCKED（必需能力事实待补）；原文 plan-observation-v1.md。
- 接受其观察任务与总目标，尚无实施 READY 或设计 PASS。评价见 plan-v1-acceptance.md。
- O1 fresh gcm session 8285；O2 fresh gcm session 23488；scope/日志/notes 见各 worker-task.md。
- 下一步：接收剩余 Native 工作/更新/关闭事实及控制事务分析，交独立 planner 补计划，不能从原生API通过直接推断 CRUD PASS。

## O2 — 身份与控制源码观察已接收

- 输入仍为 3dfdaf8503b7a6f6a76651a1e282c038b6648c3a。完整结果归档 identity-control-observation.md；只读源码证据，不是 live PASS。
- 确认客户端先拒绝 observed/provided 冲突；wire 缺批准目标/scope/代际字段。批准入口需 daemon 处理，不能依赖旧 credential 或 me()。
- 同锚点 authoritative credential 恢复被本地 runtime receipt 的提前返回挡住；后续 Register 报 TOKEN_MISMATCH。
- WorkerClosed 仅退 worker/keepalive/idle，未退 binding/route/grant/subscription；PR17 Missing 关闭修订保留。
- 现有 Subagent Record 默认 managed 语义，不能直接用于普通独立 peer 创建；复用时需显式生命周期归属与角色区分。
- 接收：owner/提交边界/源码事实；typed 输入、Update 和退役事件组合仍为提案，交 O3 与新 planner 裁定。外层 context 跨 Register、route、local receipt、projection 多边界，不宣称原子事务。
- O1 的 child AppServer 启动遇到 Operation not permitted，尚无 nonce/update/close 运行证据；待其终态后纠正执行环境，不能从启动失败推断宿主不支持。

## O1 — controller Native 运行观察补证

- 独占 `/native-owner/` receipt 证明 GCM nonce 回合可从 `thread/read` 读回、cwd settings update 对真实后续 shell 回合生效、active turn archive 后变 interrupted 且 sibling 继续可工作、同 home 的第二 endpoint 可 resume 已有且 materialized 的 session 并完成另一回合。
- Receipt 原脚本将立即陈旧 `thread/read` 错判 update FAILED；按同一原始 response sequence 已校正为更新生效。初始 worker receipt 状态保持不改，避免篡改运行证据。
- Empty/unmaterialized thread start 丢响应后没有稳定 thread 查证证据。project idempotency 不解决 thread creation idempotency。O1 worker 的 post-review receipt 也确认 project-filter thread list empty、idempotency absent；两边一致判 A6 创建 unknown 边界 INCOMPLETE。
- 当前源 Skill/目标只保留“Native 可用”的严格限制：以上不证明 Collab binding/route/grant/CRUD。O1 worker 仍在独立核验 api-supported semantics 与完整收尾；其记录由它的 owner维护。
- 第二轮独立 planner v2 已由 fresh OAuth/gpt-6.1-sol session `41878` 启动，输入 `observation-v2.md` 和 O2/O1 结果；等待完整结果后再做 parent acceptance。它与 O1 不共享写入路径。

## T1 — v2 计划裁定

- 计划：`plan-v2.md`，由 fresh OAuth/gpt-6.1-sol planner 对 `observation-v2.md`、O2 源码观察和 Native 原始回执独立规划。
- 裁定：接受 v2 的目标、分段依赖、失败边界及“F1 不依赖 peer Create unknown”修订；保持用户四项能力与 A1–A12 原验收不变。
- 首个交付：只推进 F1 CLI/MCP subagent action 的实际派发与非空错误传播。先完成独立 F1 图/设计准入，通过后才改产品代码。
- 未冻结提案：批准 JSON/wire、peer lifecycle 参数/receipt、record 扩展、跨 owner Close 顺序仍须对应设计包审查后接受；不能按 plan 提案直接编码。
- 风险/阻塞：A6 创建丢响应后的唯一宿主关联仍 BLOCKED；不阻塞 F1。O1 child 最终结果未接收，父方 Native 观察不替代其回执，也不替它清理资源。
- 图注册核对：当前 `rust/src/dagpipe.rs` 将文档图按 `manifest.json`、embedded paths、design IDs 与 operator inventory 四处闭合；D1 必须改这四处并提供非零 graph/Rust 验证。注册 operator 仅用于合同图拓扑校验，不代表业务行为落地。
- 当前工作状态：接受 v2 的 F1 增量，进入 D1 图修订与独立设计准入；F1 实施、完整交付均未通过。

## DR1 — F1 设计专审失败与重规划

- Review task：`collab-f1-design-review-20261008-v2`；Codex `oauth/gpt-6.1-sol`；terminal `fail / code_failure`，两项 P1。最终回执：`/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/.agent-collab/review/collab-f1-design-review-20261008-v2/review.final.md`。
- P1-1：CLI `me()` 不是只读 receipt 查询；它调用 daemon `IdentityContext`，可完成身份解析、Register、persist receipt 后才进 subagent action。原设计漏了提交边界。产品范围未变，但需补观察、合同和前置副作用断言。
- P1-2：`subagent::notify` 把失败 `Resp` 的 `data` 丢弃；`Ready` 丢弃成功通知的 repair projection。Send/Ready 已持久化 durable/status 后，CLI/MCP 无法报告完整 partial/repair 字段。单纯 CLI/MCP 接线不足。具体源码证据与候选最小修复边界见 `f1-review-rework-observation.md`。
- 决定：不启动 I1；I1 task contract 仅为候选，尚未派发。F1 计划范围需要允许进入 subagent result owner 的最小修复，且新增已登记/未登记路径的外部断言。保持 A1–A12 与四项目标不变。
- 下一步：以 `f1-review-rework-observation.md` 为新输入交 fresh 独立 planner，修订 F1 责任/allowed paths/测试和设计准入；收到并接受后更新图/合同，再以新 task id 重新设计 review。上次 fail 原样保留，禁止用 retry 绕过 code finding。

## T1 — planner v3 接收与局部阻塞

- Planner：fresh OAuth/gpt-6.1-sol `planner-f1-rework-v3`；输入包括 DR1 两项 P1、新源观察、plan-v2、当前 D1 图/合同、用户目标和项目规则。正文已由父保存到 `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/planner-v3/plan-v3.md`，节点笔记与结果同目录。
- 裁定：接受其源事实、P1 修订方向、精确停止条件及“不降低四项目标/A1–A12”。plan-v3 总体为 BLOCKED，但先执行只读 O4 managed fixture seam 观察，再由 D1-R3更新图/合同；不接受其禁止本轮派单的建议为总体停工。
- 已确认实施边界：需保留 `Resp.error + Resp.data` 经 `notify -> handle_with_env -> CLI -> MCP`；Ready 成功 repair projection 也要返回。`subagent.rs` 是必需 owner。`me()` 的身份协调与子代理动作按分阶段结果描述。
- 局部阻塞：O4 尚无证据证明受支持的隔离 managed fixture 能通过真实 CLI/MCP 准备 Send/Ready 前置并公开读回 durable message/status/repair/consumption；因此 B3–B5 与完整 F1 实施仍 BLOCKED。不能用私写 Record/journal、mock action 或 A6 Create 补链。
- 当前仍未实施。旧 review `collab-f1-design-review-20261008-v2` FAIL 不可变；新 D1 设计完成后须以新 review task 重审。A6 creation unknown、O1 child 接收也继续分开跟踪。

## O4 — managed fixture 支持路径观察（complete）

- Worker：fresh GCM `observe-f1-fixture-v3`，PID 10667，read-only；记录目录 `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/observe-f1-fixture-v3`。
- 仅检查生产/public managed record 创建/绑定、现有 test fixture 的真实性、实际 CLI/MCP 与隔离通知 RPC fixture 能否共同覆盖 Send/Ready，以及公开 durable 状态读回；禁止改文件、写状态、创建 peer、daemon/runtime/cleanup。
- 观察完成：production/public managed create 与真实 CLI/MCP managed Send/Ready fixture 在该基线上为 `UNSUPPORTED`；CLI mutation 未派发，且无受支持 managed-record 创建路径。公开 message/task readback 有能力，repair/wake projection 无公开 durable readback。
- 完整结果已由父编排者保存至主任务树 `docs/evidence/collab-context-identity-peer-crud-remediation-20261008/o4-managed-fixture-observation.md`。只读源码/fixture 分类，没有运行 fixture 或改动产品/daemon state；运行时可达性保持 `UNVERIFIED`。
- 决定：O4 证明既有 F1 单独实现的 fixture 前提不存在，须 fresh independent planner 重新裁定依赖顺序与最小设计边界。不得私写 Record/journal、把 ordinary peer 当 managed child、或以 A6 Create 旁路；A6 仍是独立验收。F1 实施未准入，DR1 原失败不变。

## O4 parent checkpoint — CLI public dispatch boundary

- 父编排者独立复核候选源码：`collab/src/main.rs:356-371` 的 `Cmd::Subagent` 只拒绝 `Start`，并对 `subagent_observe_query` 返回的 List/Status 调用 `Req::SubagentObserve`；其他 `Action` 分支直接返回 `Ok(())`。`collab/src/main_context.rs:358-366` 的 query classifier 只映射 List/Status。生产 CLI 没有构造 `Req::Subagent`；该请求目前只在服务端 owner (`server/mod_parts/part_11.rs:41-49`) 与 tests 中出现。
- 结果：F1 首个公开断点比“丢失错误数据”更靠前——当前 CLI mutation actions 未派发，产生无输出成功。之前 review 对后续 `subagent::notify` data 丢失的 P1 仍成立，是接通派发后的下一处语义断点。
- `subagent::Action::Start` 在 CLI 明确返回 `MANAGED_SUBAGENT_UNSUPPORTED`；生产 `Dispatch` 仅复用已登记的 managed subagent，找不到时拒绝；没有发现生产 peer/subagent 创建路径。O4 完整观察支持此结论。
- 本 checkpoint 不改变原 DR1 FAIL，不给 F1 实施准入，也不改变 A1-A12。worker 已退出；父方保存完整回执。不操作其他 worker 的目录或进程。

## T1 — fresh dependency replan v4 (accepted)

- Planner: fresh `codex exec --profile oauth --model gpt-6.1-sol --sandbox read-only --ephemeral`; session 53821, exit 0. Input and event evidence are in `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/planner-v4/`.
- Complete result saved in the main task tree as `docs/evidence/collab-context-identity-peer-crud-remediation-20261008/plan-v4.md`; acceptance in `plan-v4-acceptance.md`.
- Accepted replan: implement F1 dispatch/response preservation before managed Create; public managed success/partial/repair black-box still depends on A6. C1 stays an unintegrated candidate until A6 and complete A9/A11 evidence. The proposed `worker create --kind` shape is not frozen.
- Current ready nodes: D1-R4 design/graph revision (no product source) and O5-R4 read-only Create correlation/result readback observation. D1 needs fresh design review PASS before I1. A6 Create remains blocked; no implementation admission has been granted. Goals and A1-A12 are unchanged.

## D1-R4 / O5-R4 — dispatched in parallel

- D1 task contract and notes: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-f1-v4/`; fresh GCM workspace-write session 43979, PID 42059 (Codex PID 42062 at dispatch). Writes limited to F1 contract/design graph/registry files.
- O5 task contract and notes: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/observe-create-contract-v4/`; fresh GCM read-only session 83749, PID 42058 (Codex PID 42063 at dispatch). Only run-owned notes/results writable.
- Disjoint scopes; both read the accepted v4 plan and preserve O4/A6 unknown boundaries. Neither worker may run a daemon, consumer, create, install, or restart.
- Status: running. No D1 validation, O5 conclusion, design review, or product implementation result has been received yet.

## D1-R4 / O5-R4 — received; design gate passed

- D1-R4 returned `READY_FOR_DESIGN_REVIEW`. Graph `appsdk-collab-subagent-command@0.2.0`, five nodes/four edges; graph validation, 13 DAG tests, fmt and diff check passed. Graph SHA256 `d0a9e12fed54bfe60220c19787e8e4eaec96b7b279d73c7b64c24b909a6ab8ea`; contract SHA256 `7fe1a7892b1e77f96a3814a2ecc85b9c5324e4a8fadd9f9`. Full worker report: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-f1-v4/result.md`.
- Design review `collab-f1-design-review-20261008-v4` and stateless retry `...-v4-r1` ended in protocol failures; raw status/final/events/logs are preserved. Milestone review `collab-f1-design-review-20261008-v4-r2` ran with `oauth/gpt-6.1-sol`, returned valid evidence, and controller verdict PASS. It reported one P2 on explicit action-scoped graph routing and repeated validation; no P0/P1. Product code was not part of this review.
- O5-R4 returned read-only evidence: no stable Native request-to-thread correlation after `thread/start` response loss; Collab has no durable started/unknown stage and no public restart-safe receipt lookup; notification repair/failure/wake facts remain response-only. A6 remains BLOCKED. Parent evidence: `docs/evidence/collab-context-identity-peer-crud-remediation-20261008/o5-create-correlation-observation.md`.
- Design PASS admits I1-R4 only. I1 worker task/record: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/impl-public-entry-v4/`; GCM session `8553`, node PID `57668`, Codex PID `57673`. No product result is received yet. Keep F1 managed success/A9/A11 INCOMPLETE until A6 and full public black-box evidence close.
- While I1 writes only its allowlist, a fresh read-only planner was dispatched for D2/D3 design (`planner-f2-f3-v1`; OAuth/gpt-6.1-sol; session `42598`, node PID `87241`, Codex PID `87242`). It may only write its own external run directory. It must keep A6 Create BLOCKED and may not edit candidate files. Result path: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/planner-f2-f3-v1/`.

## T2 — F1 checkpoint refreshed; D2/D3 v2 planning accepted for design only

- F1 now has `subagent::tests` 10/10 and full Action routing test 1/1; public daemon-backed CLI/MCP remains blocked by AF_UNIX/loopback bind `EPERM`. See the refreshed F1 checkpoint; do not count local logic tests as live success.
- Fresh OAuth/gpt-6.1-sol read-only planner output: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/planner-d2d3-v2/plan.md`; invocation event stream and stderr are in the same directory. Exit code 0. The plan is READY only for minimal remaining observation and D2/D3 contract/graph design.
- Parent acceptance: `docs/evidence/collab-context-identity-peer-crud-remediation-20261008/plan-d2d3-v2-acceptance.md`. Reuse O2/O5; only inspect uncovered owner/receipt/retirement semantics. No D2/D3 product implementation is admitted. Existing dirty registry files remain owned by their current writer and must not be overwritten.
- A6 Create stays BLOCKED. D2/D3 design reviews, runtime validation, implementation, install/live, implementation review, integration, and cleanup remain open. Goal is active / INCOMPLETE.

## D2 / D3 — contract and graph design dispatched

- Before dispatch, current process scan showed no appsdk planner/worker writing this task. Existing `docs/dagpipe/manifest.json`, `rust/src/dagpipe.rs`, and `rust/src/dagpipe_tests.rs` changes trace to the D1-R4 allowlist in `design-f1-v4/worker-task.md`; current diff is only D1 graph registration. Parent accepted sole G23 ownership. This supersedes the earlier assumption that the files may belong to another task.
- No native Codex thread-bridge tools are exposed in this run. Following the Codex Desktop worker route, two fresh GCM workers were started with disjoint write scopes:
  - D2 `design-d2-identity`: node PID 84143; output/notes `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-d2-identity/`; allowed D2 contract, anchor restore design, context graph, pane-route graph. No registry writes.
  - D3-RUC `design-d3-peer-lifecycle`: node PID 84142; output/notes `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-d3-peer-lifecycle/`; allowed new lifecycle contract and graph. Create remains blocked. No registry writes.
- Both sessions were confirmed running by `ps`; task contracts and initial notes were written before dispatch. Parent owns registry integration after both workers stop. No product source or runtime/identity operation was dispatched.

## D3-RUC — stale read-only worker interrupted

- Parent checked `ps`, notes, candidate status, and the worker event stream. The D3 worker had produced no new event for about 13 minutes, had no product/design file changes, and remained at 0% CPU while the last source-read command had completed.
- Parent sent SIGINT to D3 Codex PID `84142`. A follow-up `ps` showed the worker and wrapper stopped. The external run directory, notes, event stream, and stderr were preserved; no candidate file was reverted or cleaned.
- D3 design is still **not delivered** and **not admitted**. The captured read-only source facts may be reused only after parent review. D2 worker PID `84143` remained active; its output must be received and checked before registry integration or any dependent design review.

## D2 — stale read-only worker interrupted

- D2 likewise stopped producing events after its source-read command completed; after about 15 minutes, `ps` showed the wrapper and Codex process idle at 0% CPU. Its run directory contained no `result.md` and no candidate design edits.
- Parent sent SIGINT to Codex PID `84143`; a follow-up process check confirmed the wrapper and worker had exited. Notes, events, stderr, and all candidate files remain preserved.
- D2 contract/graph design is **not delivered** or admitted. Any continuation must use the frozen input facts and new run identity, avoid replaying already-completed environment probes, and resume only at source-owner mapping/design edits.

## T1 refresh — fresh planner dispatched

- Parent wrote `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/latest-observation-20261009.md` from current main/candidate hashes, dirty paths, installed CLI path/version, read-only Collab status, worker process state, accepted plans, and valid prior evidence. It distinguishes facts, parent judgment, and unknowns; no identity or daemon write operation was run.
- Fresh planner run: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/planner-resume-20261009/`; invocation is `codex exec --profile oauth --model gpt-6.1-sol --sandbox read-only --ephemeral --json`, thread `01a11f15-181c-7e01-a9e1-4224da8f5897`, CLI session `95906`, wrapper PID `5607`, Codex PID `5611` at dispatch. Its only write path is its own plan output/events/stderr; no product code or task docs are writable in the `read-only` run.
- At this note update the planner had read the latest observation, complete task source, accepted plans and original evidence; `plan.md` had not yet been returned. No planner conclusion or acceptance is recorded.

## T1 refresh — planner received and accepted for D2-A only

- The same planner session returned `plan.md`, `turn.completed`, and process exit `0` after one logged transient response-stream disconnect/retry. The process/session was polled through its original handle; it was not restarted. Full stdout/output is preserved in `planner-resume-20261009/`.
- Parent verified `git ls-remote origin refs/heads/main` still equals candidate HEAD `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`.
- The full plan was copied byte-for-byte to `docs/evidence/collab-context-identity-peer-crud-remediation-20261008/plan-resume-20261009.md`; SHA-256 `e8ce1907a6b63eb5fb9dea4cca1177cd6da214dec4bf6939fe2e5be3b5e3f5a4`. Parent accepted `READY` only for the bounded D2-A read-only admission/receipt/query map. D2-B, D3-RUC, implementation, A6 Create, install, and runtime mutation remain unauthorized.
- Controller refinement: do not edit `W/note.md` for a duplicate task summary; the Goal and its evidence `run-notes.md` remain the task truth. This does not alter the D2-A source mapping scope.

## D2-A — read-only receipt/admission map dispatched

- Task contract: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-d2-admission-v3/worker-task.md`; scope is only the six requested source-owner/admission/receipt/query questions. It cannot write candidate files or run product/runtime commands.
- Fresh GCM Codex invocation uses `--sandbox read-only`, candidate cwd `/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008`, and output path in the isolated D2-A run directory. CLI session `35112`; wrapper PID `91862`; Codex PID `91908` at initial poll. `events.jsonl` began, but the first useful evidence is not yet returned; result file is pending.
- Parent owns progress monitoring against the accepted 3-minute stage-output / 5-minute no-new-evidence diagnostic contract. A timeout alone will not trigger a duplicate dispatch or imply failure.
- Three-minute checkpoint: the exact wrapper/Codex PIDs were still live; events confirmed only reading the task contract/prior files and checking the candidate status. No focused receipt source read or map had been recorded yet. `stderr.log` shows MCP initialization failures/retries (`mcpx` loopback endpoint refused and `routecodex-hooks` disconnected); they are not treated as a product-source result. Continue polling this exact handle until the 5-minute diagnostic before considering interruption.

## F1 source audit — split Send partial commits

- Parent read the live candidate implementation and found that Send notification failure reported only one `state_commit`, although `working→idle` may already have committed before notification and a second error-record commit happens afterward.
- Corrected `subagent.rs` to report `state_commit` for the pre-notification transition and `error_state_commit` for the notification-error record. The response now uses the re-read current status and preserves durable `msg_id` plus the original complete notification `Resp`.
- Verification after the correction: `subagent::tests` 10/10 PASS; `subagent_public_entry_cli --no-run` PASS; rustfmt on changed Rust files PASS; `git diff --check` PASS. The daemon-backed consumer is still not run because the local socket bind restriction remains.

## D2-A — receipt/admission map accepted; D2-B admitted

- D2-A worker returned a completed read-only source map at `docs/evidence/collab-context-identity-peer-crud-remediation-20261008/d2-admission-receipt-map-20261009.md`; SHA-256 `d708e33e3e4573987fc09ae71280c98c7649f33499b7ddd6c764ce02f2a45918`. The worker thread completed; do not infer an unobserved CLI exit code.
- Parent acceptance is recorded at `docs/evidence/collab-context-identity-peer-crud-remediation-20261008/d2-admission-receipt-map-acceptance-20261009.md`. Verdict is `READY_FOR_CONTRACT` for D2-B design only. No product implementation, D3-RUC, Create, installation, or runtime identity action is admitted.
- Source map confirms that IdentityContext precedes ordinary route admission; nested Register receipt is not an outer restore receipt; CommandStarted is not queryable and incomplete replay fails; normal context query requires the old credential; route, local credential, grant, lease, and snapshot are separate commit boundaries. Do not fabricate `actor_binding_id`; identity and master grant remain separate owners.
- Next: D2-B must freeze the stable pre-side-effect operation identity, phase/replay/readback semantics, approval ownership, and expired-credential-independent query; update the anchor/minimal-interaction conflict and context/pane-route graphs; run static graph/document checks and return the exact candidate diff/SHA for independent design review. D3-RUC remains downstream.

## D2-B — approved identity recovery contract/graph design dispatched

- Before dispatch, synced the candidate copy of the task truth from the main-tree task file; appended the D2-A receipt/acceptance and exact next gate to both run-notes. Existing F1/DAG dirty changes remain intact.
- D2-B worker contract: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-d2-identity-v4/worker-task.md`; notes: same directory `notes.md`. Allowlist is only the identity minimal-interaction contract, anchor restore model, context graph and pane-route graph. Registry and product files remain excluded.
- Dispatched fresh `codex exec --profile gcm --json --sandbox workspace-write` from candidate `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`, unified exec session `11787`, at `2026-10-09T05:39Z`. Startup logged `routecodex-hooks` MCP initialize closed and loopback MCP `127.0.0.1:9090` unavailable/retrying; these are runner initialization issues, not product evidence. Continue polling this same session and preserve its exact outcome.
- D2-B v4 worker made no allowlist edits and returned no design artifact. After about 6 minutes, multiple shell calls failed with `Failed to create unified exec process: No such file or directory` despite isolated calls succeeding intermittently. Parent sent SIGINT to exact owned Codex PID `61177`, then Ctrl-C to unified exec session `11787`; session exited 1. Follow-up `ps` and allowlist status confirmed process termination and no D2-B design edits. Original worker run and output are preserved; this is an interrupted attempt, not a product/design failure verdict.

## D2-B — constrained continuation dispatched

- Fresh worker contract and notes: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-d2-identity-v5/`. It inherits no conversation, binds the same accepted D2-A and candidate HEAD, restricts writes to the four existing design files, prohibits parallel shell commands, and limits tool failure diagnosis to one `pwd` attempt before reporting blocked.
- Fresh constrained D2-B worker dispatched in session `93341` at `2026-10-09T05:45Z`; startup again logged unavailable loopback MCP `127.0.0.1:9090` and routecodex-hooks initialization failure. These remain runner warnings only.
- D2-B v5 was stopped before any design read/edit after the worker identified that the copied worker contract still pointed to v4's notes path. Parent sent Ctrl-C to unified exec session `93341`; exit code 1; process scan found no worker and allowlist remains unchanged. Corrected worker ID and all run paths in fresh v6 contract at `.../design-d2-identity-v6/`; v4 and v5 notes are retained. This is a parent dispatch-contract correction.
- Corrected D2-B worker `design-d2-identity-v6` dispatched fresh in session `20217` at `2026-10-09T05:47Z`. The worker contract path/ID was checked before launch. Startup still reports unavailable loopback MCP; no product/runtime work depends on it.

## D2-B — contract and graph candidate delivered; independent review pending

- Fresh worker `design-d2-identity-v6` returned `READY_FOR_DESIGN_REVIEW` and exited normally. Its records are `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-d2-identity-v6/worker-task.md` and `notes.md`.
- Candidate allowlist has exactly four files: `docs/design/collab-identity-minimal-interaction.md`, `docs/design/collab-anchor-restore-model.md`, `docs/dagpipe/collab-context.graph.json`, and `docs/dagpipe/collab-pane-route-reconcile.graph.json`. SHA256 values are in worker notes and verified at delivery.
- Design adds four typed context invocation kinds, operation identity before side effects, phase/owner/replay/query contract, scoped approval distinct from endpoint ownership and master grant, and partial/unknown/failed/denied/cancelled/completed outcomes. Anchor and pane-route conflicting text was revised. No product code, registry, runtime or identity state was touched.
- Author checks: JSON parse passed for both graphs; context graph v0.10.0 passed DagPipe validation at 6 nodes/5 edges; pane-route graph v0.6.0 passed at 5 nodes/4 edges; `git diff --check` exit 0. `dagpipe graph validate` does not establish project `compile()` or behavior.
- Independent milestone review `appsdk-d2-identity-design-review-20261009-v1` started with `oauth/gpt-6.1-sol`, mode `uncommitted`, bound to this exact four-file candidate. Review is pending. D2-B is not yet accepted; no F2 implementation admission. A6 remains BLOCKED.

## D2-B v6 review — rejected on six design P1 findings

- Independent milestone review `appsdk-d2-identity-design-review-20261009-v1` completed with controller `FAIL / code_failure / blocking_findings`; the six P1 findings are in `/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/.agent-collab/review/appsdk-d2-identity-design-review-20261009-v1/review.final.md` and controller receipt from `review_result`.
- Findings are design-only: (1) approved stale-credential recovery has no implementable admission/rebind seam; (2) nested Register correlation is recorded only after dispatch, leaving a crash window; (3) query schema lacks caller proof fields required by the contract; (4) approved CLI/MCP schema and expected-binding/generation fence are unspecified and old wire schema conflicts; (5) query is described both as repairing and as read-only; (6) invocation results and retained response schemas contradict each other on `partial`, `failed`, and `approved`. No finding attributes failure to tmux tests; review explicitly says behavior tests are out of scope for this design gate.
- Parent has updated the task truth to mark v6 rejected and v7 correction active. The exact findings are supplied to fresh worker `design-d2-identity-v7`; same four-file allowlist, no product implementation or runtime action.
- D2-B review correction worker `design-d2-identity-v7` dispatched fresh in session `42970` at `2026-10-09T06:26Z`. The task is limited to resolving the six P1 design findings in the same four-file allowlist. No product implementation or tmux/runtime test work is assigned.

## T2 — tmux public-entry blackbox rerun

- User correction: tmux-based testing is an available background execution path and must not be reported as a blocker by itself. Parent launched the existing `subagent_public_entry_cli` public-entry integration target in a dedicated tmux session `appsdk-collab-t2-20261009`, from candidate `/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008`. This test target owns its isolated host state and private tmux fixture; it did not reuse or mutate the user's daemon/session.
- Exact command: `cargo test --manifest-path collab/Cargo.toml --test subagent_public_entry_cli -- --nocapture`.
- Result: exit code `0`; 4 passed, 0 failed, 0 ignored; elapsed 3.03s. Coverage includes Start refusal before identity coordination, MCP empty/invalid success rejection, List/unknown Status with isolated daemon, and missing-child mutation errors across CLI/MCP. The previous AF_UNIX `EPERM` did not recur.
- Raw output: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/t2-tmux-public-entry-20261009/test.log`; SHA-256 `a8f2876602e4bd7550d79554b7ac0f2a4b9633168f06b9dad49921888400228b`; exit receipt is the sibling `exit-code` file (`0`).
- Status correction: T2 remains IN_PROGRESS pending remaining F1 diff review/closure evidence, but daemon-backed CLI/MCP blackbox is now PASS and no longer classified as blocked by socket permission. This does not change D2-B v6 review verdict or authorize D2-B-dependent implementation.

## T2 — F1 parent diff review and closure

- Parent reviewed the current candidate changes in `collab/src/main.rs`, `collab/src/subagent.rs`, `collab/src/bin/collab-mcp.rs`, the focused tests, and the new public-entry test target. The mutation wrapper emits the daemon result; List/Status take the observe route; Start fails before identity coordination; mutation errors preserve typed response data and report state commits; MCP rejects empty and invalid JSON from successful subagent calls. No blocker was found in the reviewed F1 scope.
- Verification evidence: earlier focused subagent unit tests 10/10, action routing test 1/1, and `cargo test --no-run --test subagent_public_entry_cli` passed; the fresh tmux public-entry target passed 4/4 at the exact path/log and SHA recorded above. `git diff --check` on the changed F1 Rust files passed. Whole-workspace `cargo fmt --all --check` is not a valid scoped gate here: the candidate root is not a Cargo workspace and it surfaced unrelated pre-existing formatting differences in other project files; no files were changed by that check.
- T2 is now **COMPLETE at the candidate author gate**. Independent review of the integrated candidate remains T7, after T6; this T2 completion does not claim final product, installed/live, merge, or Goal completion.
- Correction record: user said “阻塞你妈，用 tmux 测试怎么会阻塞”. The first divergence was treating an earlier `EPERM` probe as a current gate without running the existing tmux integration target. The new tmux run passed and changes the state: T2 is closed and the specific public-entry test is no longer blocked. The durable task row and summary were updated; no global rule change is needed because the project run record now captures the concrete command and evidence.

## D2-B v7 — live-session checkpoint

- At `2026-10-09T06:43:27Z`, the exact existing wrapper/Codex PIDs `50195/50241` and session `42970` were still live. I polled the same session handle multiple times; it remained open with no new output during those polls. Its run note was last updated at local `23:36:58`.
- Current SHA-256 for the four allowlisted design files still exactly matches the rejected v6 values recorded in `design-d2-identity-v6/notes.md`: minimal interaction `813e679c…65d38`, anchor restore `88d615cd…24c31`, context graph `bdced8f6…7404d`, pane-route graph `14fc683e…88674`. Therefore v7 has not yet produced a design edit or reviewable candidate at this checkpoint.
- The worker remains alive, so this is a verified wait, not a terminal failure or blocked verdict. Keep polling this same handle; do not start a duplicate writer or admit implementation. T2 is complete at its author gate; overall Goal remains active / incomplete.
- At `2026-10-09T06:52:39Z`, polling session `42970` showed the worker had entered its four-file patch sequence. The Codex event stream reported deletion of `docs/design/collab-identity-minimal-interaction.md`; the subsequent replacement event has not yet arrived, and a fresh filesystem check confirms that file is temporarily absent. Wrapper/Codex PIDs `50195/50241` remain live at elapsed 25m41s. Preserve the same owner/session and continue polling before reading or editing any of the four design files; do not restore v6 over the worker's in-progress replacement.
- Recovery-source check: the candidate review/run directories contain v6 worker notes and review output, but no separate patch, diff, or snapshot copy of the deleted design file. Since the owning worker is still live, preserve the exact session and wait for its write instead of replacing the file from the base branch or guessing the rejected v6 content.
- A read-only search found the baseline `collab-identity-minimal-interaction.md` in the main tree and several other AppSDK worktrees, all SHA-256 `5dc74090219ceeee2a11f9f1c2827d948ff30404a4afea22de7b424a60ca8f3f`. This is a recoverable pre-v6 baseline, not a copy of the rejected v6 contract. Do not restore it while session `42970` is live; retain this hash as a fallback only if the worker reaches a confirmed terminal state without replacing the file.
- At `2026-10-09T06:56:49Z`, session `42970` logged `responses_retry: stream disconnected`, retry `1/5`. Subsequent polls returned no event, while the exact processes remained live (elapsed 30m53s) and the contract remained absent. This is a transient runner-stream failure, not a design verdict; continue observing that same session through the retry result.

## D2-B v7 — terminated and contract restored

- At `2026-10-09T07:03:48Z`, after repeated same-handle polling confirmed more than 30 minutes with no useful output, the contract still absent, and only the stream retry warning, parent sent Ctrl-C to owned session `42970`. It exited `1`; follow-up `ps` confirmed PIDs `50195/50241` were gone. Shutdown also logged loopback MCP `127.0.0.1:9090` initialization failures. No v7 deliverable was returned.
- After confirming worker termination, restored only `docs/design/collab-identity-minimal-interaction.md` from `/Users/fanzhang/Documents/github/appsdk/docs/design/collab-identity-minimal-interaction.md`, verified SHA-256 `5dc74090219ceeee2a11f9f1c2827d948ff30404a4afea22de7b424a60ca8f3f`. The other three D2-B files were not changed by recovery and remain v6 pending v8 correction. This restores a valid baseline; it does not accept v6 or satisfy any of its six P1 findings.

## D2-B v8 — fresh correction worker dispatched

- New run contract and pre-dispatch notes: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-d2-identity-v8/worker-task.md` and `notes.md`. It retains the same six P1 findings and four-file allowlist, documents the v7 terminal state, and requires writing the full contract under the run directory before atomic rename over the candidate file.
- Fresh isolated command: `codex exec --profile gcm --sandbox workspace-write --ephemeral --json`, candidate cwd `/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008`, run dir `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-d2-identity-v8/`; unified session `57200`, wrapper PID `5622`, Codex PID `5624`; thread ID `01a11f7a-e1d5-7e43-bd65-fdb10c98d200`.
- Initial process scan confirmed the new wrapper/Codex and event log. No product/runtime/test action was started. T3 remains IN_PROGRESS and no identity implementation is admitted pending v8 delivery and fresh independent D2 review PASS.

## B23-O parent review correction and K3 parallel contract (2026-10-09)

- Rechecked the accepted B23-O candidate and its latest source/tests while the independent read-only audit was live. Found `conn_task_degraded` called a nonexistent `write_response` helper in the malformed-query branch, and healthy dispatch only classified `query`/`invocation=query`, while degraded dispatch and activity filtering also recognized `action=query`.
- Corrected the B23 allowlisted candidate only: `part_12.rs` now centralizes query intent/strict-shape checks; `part_04.rs` returns the same typed invalid-shape response in healthy mode; degraded mode constructs and writes a normal typed `Resp` through its existing loop. The public request-activity path treats any query marker as query so malformed query capabilities are not written to activity logs.
- Added daemon-backed assertions to the existing operation-contract test target for malformed `action=query` in both healthy and project-replay-degraded startup. The assertions require typed rejection and unchanged project/host journal bytes; healthy mode also verifies no activity-log change.
- Final evidence is under `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/implementation-b23-operation-journal/tests/query-shape-fix-20261009/`: focused `server::operation_contract_tests` 12/12 pass (exit 0); `cargo test --manifest-path collab/Cargo.toml --locked --bin collab --no-run` exit 0; `git diff --check` exit 0; `rustfmt --check --edition 2021` passes for the new `operation_contract_tests.rs`. Full package `cargo fmt --manifest-path collab/Cargo.toml -- --check` exits 1 due numerous existing candidate formatting diffs across unrelated/earlier-dirty files; the new test file was formatted, and changed dispatch sections have no rustfmt diff. Original pre-correction logs were preserved beside the rerun.
- B23 is still candidate/in progress, not accepted: independent observer session `70724` is live and writes only `implementation-b23-independent-audit/`. It must return source findings before parent accepts or dispatches shared-path S23/D2/D3 implementation.
- Dispatched independent read-only K3 peer lifecycle host-capability contract worker `appsdk-contract-k3-peer-ops` in unified exec session `58962`, exact contract and task binding in `implementation-k3/worker-task.md` and `dispatch.json`. It reads accepted R/U/C and Native receipts, writes only its own evidence folder, runs no host experiment, and does not start lifecycle implementation or Create/A6.
- Goal remains 3/9 complete, 2/9 in progress, 4/9 not started; T3/T4 remain in progress. No install, daemon restart, identity/peer mutation, Git integration, or resource cleanup was performed.

## B23-O audit findings closed on current candidate; independent r2 pending (2026-10-09)

- Independent audit r1 completed exit 0; parent captured its final message verbatim at `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/implementation-b23-independent-audit/result.md`. The worker's requested run-directory file writes were denied by its `--sandbox read-only`; no bypass was attempted. Its concrete findings were crash projection `phase=unknown` retaining durable history, inconsistent healthy/degraded query marker validation, and raw `query_capability` in healthy activity logs. The candidate has since changed; r1 is historical and not an acceptance verdict.
- Cross-check against accepted D2-B §3.1 showed `committed_phases` must contain only durable producer-receipt-backed phases and never `admitted`; current effective `phase=unknown` after an incomplete tail is an inferred crash outcome, while the list retains the last durably committed business phases. Aligned `operation_journal.rs` admission/replay validation to omit `admitted`; incomplete-tail query now returns `phase=unknown`, outcome `unknown`, and committed `[validating, inner_dispatched]`. Test asserts that exact projection after restart. This closes the semantic finding without changing the accepted contract.
- Query fixes: shared query-marker/strict-shape helpers now gate healthy and degraded paths; degraded malformed requests return typed `Resp` using the existing write loop; healthy connection activity skips any query-marked request. Tests assert malformed action-only query gets `IDENTITY_OPERATION_QUERY_SHAPE_INVALID`, journal bytes do not change, and healthy events log is unchanged.
- Final candidate file hashes at r2 dispatch are in `implementation-b23-independent-audit-r2/worker-task.md`. Latest author evidence: `tests/crash-unknown-projection-20261009/operation-contract-tests.log` -> 12/12, exit 0; `collab-no-run.log` exit 0; `git-diff-check.log` exit 0; `rustfmt --check` on `operation_journal.rs` and `operation_contract_tests.rs` exit 0. Full package fmt check remains nonzero from broad pre-existing formatting differences; its full log is in `tests/query-shape-fix-20261009/cargo-fmt-check.log`. The changed dispatch hunks introduce no additional rustfmt output; no unrelated file was reformatted.
- Fresh independent audit r2 dispatched read-only in unified exec session `28629`, bound to exact source hashes; it returns a final message which parent will save verbatim. It does not run product tests or mutate candidate. K3 independent host-capability contract remains active in session `58962` in its separate directory.
- B23-O remains pending parent acceptance until audit r2 and current diff are assessed. No S23/D2/D3 shared writer dispatched. Goal remains 3/9 complete; T3/T4 in progress; T5–T8 not started.

## Current worker delivery recheck (2026-10-09)

- Revalidated the live GCM handles after the prior progress turn: B23 audit r2 (`28629`) and K3 (`58962`) are still running and their JSONL event files continue to grow; neither has a terminal `worker.exit` yet.
- K3 emitted a progress message saying its `result.md` and `candidate-binding.json` were written, but inspection of the contracted output directory showed only `dispatch.json`, `events.jsonl`, `notes.md`, `stderr.log`, and `worker-task.md`. Its subsequent `jq empty candidate-binding.json` command exited 2 because the relative file is absent. Treat K3 as IN_PROGRESS with a delivery-path mismatch; do not accept its announced result until absolute-path artifacts exist, parse, and match the contract. Candidate has not been modified.
- B23 audit r2 has not returned a final message or exit code. Its latest progress states the crash projection semantics are closed and it is checking query schema/secret/activity boundaries. B23 remains candidate/in progress pending that report.

## T4/T5 candidate acceptance and T6 handoff — 2026-10-10 14:33 UTC

- T4 candidate behavior acceptance is complete: the previously recorded A6 Create 8/8, shared peer-lifecycle reducer/public contract 29/29, and R/U/C focused cases remain unchanged and pass. The final isolated-daemon public adapter target `cargo test --manifest-path collab/Cargo.toml --locked --test peer_lifecycle_cli -- --test-threads=1` passed 2/2, covering master context operation-card readback, CLI Create/Read/Update, MCP Create/Read/Close, and frozen CLI help.
- T5 is complete on candidate source: master `collab context` emits exact Create/Read/Update/Close/Query commands with same canonical `main` + app-scope constraints and terminal/failure guidance; MCP catalog describes Create and first-call versus retained retry operation IDs; embedded source Skill describes trigger, parameters, required receipts, and unknown/refused handling. The exact public context assertion is part of the passing isolated-daemon test.
- The peer adapter test initially misused `--op`/`operation_id` on first Update/Close attempts. Those fields are retained operation IDs for retry/query; first Update/Close omits them. With that fixture correction, Update returned `complete` only after the simulated subsequent cwd turn was read back. The fixture's `completed_turn` behavior now also emits Create readiness evidence.
- A targeted `collab` unit-test build exposed stale test-only initializers missing the newly required protocol defaults `IdentityContextRequest::invocation_ticket` and `OperationAdmission::approval_evidence`. Added empty/None defaults only to test constructors. `context_gives_an_idle_master_one_canonical_scheduling_action` passed 1/1; `server::operation_contract_tests` passed 13/13.
- `rustfmt --check --edition 2021 collab/tests/peer_lifecycle_cli.rs` and `git diff --check` passed. A package-wide rustfmt check was not rerun; previous candidate history records broad pre-existing formatting differences, and touched legacy source files are not reformatted wholesale.
- Goal count is now 9 nodes: 6 complete (T0–T5), 0 active, 3 not started (T6–T8). Formal install/runtime, production identity/peer writes, independent milestone review, Git integration, remote receipt, and resource cleanup have not happened.
- Next owner is the parent migration owner for T6. Read `appsdk-migration` and Collab verification references, create the exact run record, inspect/classify current daemon/resources, snapshot and freeze through the official migration route, then use `scripts/install-global-collab.sh`, one controlled daemon maintenance window, and installed context/MCP/live acceptance. Do not clear/promote production master or alter peer identities as part of installation.
