# AppSDK 权威需求 review 模板：编排记录

2026-10-07 / scope｜用户要求 AppSDK 治理拥有 review 模板，agent 根据权威需求组装完整审查材料；使用新建 gcm worker 编排执行。当前增量先交付 SDK 模板、需求上下文组装及 review 准入/记录接线；不以当前全局 Skill 修改代替产品能力，不擅自扩大到新的鉴权服务｜用户本轮指令；主树设计交接｜先确认真实流程和能力，并独立设计准入后实现。

2026-10-07 / baseline｜origin/main 与主树 HEAD 均 c3c0c8df79e69534fe30c92db61328824473d5c0；主树仅本任务设计 dirty，保留。自有独立 worktree 已创建｜git fetch/worktree 回执｜branch codex/authoritative-review-template-20261007；worktree /Volumes/Intel/playground/appsdk/authoritative-review-template-20261007；codex-cli 0.160.1｜并行只读 W1/W2，分别负责 review 资源接线和权威需求证据/验收。

资源 owner：本目标编排者；自有 worktree 与 .worker-runs/authoritative-review-template-20261007 临时记录在交付或明确结案后保留必要证据再移除。不得清理 collab-identity-shortest-path-20261007 或其他资源。merge/push、正式候选及验收由 parent 收口。

2026-10-07 / worker 启动｜W1/W2 同批使用裸 codex exec --profile gcm --json，独立日志与笔记；CLI session 21732 / 88324｜w1/events.jsonl、w2/events.jsonl｜read-only 合同，workspace-write 只为独占笔记可写；未传父 session/transcript｜读取报告后冻结当前增量设计。

2026-10-07 / owner 纠正与候选输入｜主树设计/goal 修订为 AppSDK 拥有模板/分发/组装/准入，global reviewer Skill 仅适配；设计交接文件已逐项复制到候选，未覆盖产品代码。canonical appsdk verify-sdk-source-registry 在候选 PASS｜SDK_SOURCE_REGISTRY 回执 {ok:true,gate:sdk_source_registry}｜base c3c0c8d，当前增量 G9/T16；既有历史 source-line blocker 不适用于本基线｜等待只读能力报告，准备独立设计 review。

2026-10-07 / intake 首次校验｜BUG_INTAKE_SCOPE_INVALID；源码 bug_cli.rs:585 要求 scope 为字符串数组，已在唯一 intake 文件修正｜intake.json 与首次错误回执｜未创建 issue，未写产品｜重试正式 intake；并行确认构建能力。

2026-10-07 / intake 与构建能力｜正式 feature issue 592e241 已创建；scope 数组合同修正后 intake PASS。cargo test --locked --no-run 编译全部 Rust 测试产物成功（exit 0）｜intake 回执；baseline-build.log｜候选 base c3c0c8d + 文档草案；独占 CARGO_TARGET_DIR 位于本轮记录/build｜报告冻结后直接进入独立设计准入，不继续全仓侦察。

2026-10-07 / 辅助命令纠正｜cargo test --lib 不适用：此 package 无 library target；没有执行测试。构建能力已由 --no-run 证明，不重复运行无效命令｜baseline-lib.log｜exit 101: no library targets found in package appsdk｜实现后的定向测试使用已确认 bin/integration 入口。

给 W1/W2 的当前收口指引：目标/基线已在笔记确认，无输入变化不重复检查。只读侦察应止于能冻结当前增量接口和公开验证入口；把剩余非必需细节写为建议即可，立即落盘 report 回报，不继续全仓扫描/哈希或重复读取完整规则。parent 已确认编译能力与 source registry，复用该证据。

2026-10-07 / DAG 修订｜候选消费 graph 0.2.0 增加唯一 assemble_review_packet 节点：绑定任务 → SDK 模板组装 → 独立需求/行为证据核验 → 准入。dagpipe graph validate PASS，6 节点5边6波｜候选 docs/dagpipe/user-requirement-consumption.graph.json；设计对应节点表｜草案绑定仍未实现；未改变权威需求写链｜报告齐备后冻结最小公开接口并独立设计 review。

2026-10-07 / W2 接收｜W2 exit 0，report.md 与逐节点 notes 均存在；已读完整报告。确认 goal 结构/弱确认、Guidance/compile 版本漂移骨架、review_id 同源重算；缺 ReviewRecord 需求版本与 SDK 组装入口。G9/T16 可复用这些 owner，G1–G8 认证/长期集合仍未实现｜w2/report.md、w2/notes.md、session 88324 completion｜代码基线 c3c0c8d，候选文档新版｜冻结具体材料/记录/准入接口；不重查未失效源码事实。

2026-10-07 / W1 取消及证据恢复｜W1 反复扫描未变源码且无节点报告，仅初始 notes；330前后事件，多次 apply_patch payload fatal 后继续广泛扫描。取消准确自有 PID 98798/98771；CLI session21732结束，未有report，不能判完成。8项已执行成功源码观察提取保存｜w1/events.jsonl/stderr.log；w1-report/source-observations.txt｜无产品代码写入｜新建 gcm W1-report 只收口现有证据，不重复审计。

2026-10-07 / 模板源并行实现｜W3-template 正在实现 SDK Skill 真源三份 Markdown，产品/Rust/JSON 禁写；Session93463｜w3-template/contract.md/events.jsonl｜已知 owner、纯文档针对性检查路径｜包装/准入代码仍等独立设计准入。

2026-10-07 / W1-report与W3接收｜W1-report exit0且report已读，确认正式source→bundle logical path→.appsdk/skills安装；current project_bindings已进入review_id，可复用；新mandatory binding为真实行为变更需更新现代fixture并保护历史。W3 exit0且三份模板Markdown/报告已读，入口已接线，未包装｜w1-report/report.md；w3-template/report.md和源码三文件｜源码base c3c0c8d｜冻结增量接口并启动W4独立设计审查。

2026-10-07 / 当前设计冻结｜新增 appsdk-authoritative-review-packet.md：review-context只读公开入口，从goal/candidate/evidence/SDK模板生成材料；新PASS要求明确reviewer上下文核验，identity持久绑定，后续gate拒绝陈旧；无新授权、无第二需求存储、frozen历史兼容｜候选增量设计、0.2.0消费graph｜接口待独立DESIGN_PASS后编码｜W4只读审设计，代码暂不写。

2026-10-07 / 编排恢复｜已读节点笔记及W1-report/W2/W3已接收事实；W4仍在独立设计审查，未有最终裁决，不启动产品代码。W5代码/W6公开黑盒合同已准备，写入范围分离｜w5-code/contract.md、w6-blackbox/contract.md｜同一候选base；接口ack拟固定requirements_review {context_id,checked}，须等DESIGN_PASS｜审查结果接收后同批执行。

2026-10-07 / goal状态校正｜完整goal为长期目标提示词，用户已要求执行；候选goal首段更新当前SDK review增量和无自动订阅事实，原全部验收不降｜候选docs/goals/user-requirement-truth-lock-goal.md｜纯文档状态，无产品行为宣称｜当前设计审查仅覆盖G9/T16切片，长期授权/持久锁仍gap。

2026-10-07 / W4设计FAIL接收与修订｜独立review确认stale architecture PASS会阻塞完整admission，从而不能生成替代context；代码前阻断成立。设计修为共用作者readiness helper，不调用下游publication检查；旧PASS在下游仍拒绝。ack接口冻结并补恢复黑盒，当前图证据已刷新｜w4-design/review.md；候选设计/template和W5/W6合同；graph-validation.txt｜W4 exit0，root notes/review已移回独占记录目录；无产品代码｜新建GCM独立复审修订，仍不编码。

2026-10-07 / 提交工具能力预查｜MCPX runtime_read/workspace成功，但未注册本候选/AppSDK根；现有Skill声明新注册须重启MCPX才刷新。当前任务无非目标共享服务重启授权，不动MCPX/旧workspace。提交/gate必要短命令走项目CLI并保存真实回执，不伪装MCPX证据｜MCPX structuredContent workspace清单；mcpx Skill Workspace生命周期｜runtime 0.9.18，候选独占路径｜此辅助缺口不阻断工程主线。

2026-10-07 / 独立设计准入PASS｜W4R完整报告已读，前P1循环依赖及P2绑定/图证据均解决；生产merge顺序必须先将reviewer ack纳入project_bindings再计算review_id；共用readiness不带下游review校验｜w4r-design/review.md、notes.md｜base c3c0c8d+修订设计；长期认证/强锁未实现｜现在同批派发W5代码/W6黑盒，范围已隔离。

2026-10-07 / W5-W6启动回执与证据归档｜W5 session33380/W6 session40613，独立codex exec --profile gcm、无父会话继承。同批合同约定生产/测试写入隔离及canonical测试consumer selector。已完成W1/W2/W3报告和W4失败/W4R通过报告逐项复制到项目evidence/execution，原始事件仍保留待结案｜execution/*.md；独占worker stdout/stderr｜W4R session30294 exit0｜实现完成后精确候选作者验证及正式架构review。

2026-10-07 / 模板输出边界澄清｜requirements_review是AppSDK architecture observation输入，不要求backend模型改固定JSON schema。模板明确reviewer在现有evidence字段返回精确context_id和核验事实，外部lifecycle adapter只能映射已收到的确认，不能造checked。本增量不新增自动模型/adapter链｜authoritative-review-template.md identity字段｜语义澄清，不变已审接口/固定JSON合同｜实现后的独立review核验此边界及实际SDK模板身份。

2026-10-07 / 公开黑盒红测接收｜W6 focused-red.log 5例FAIL，实际同入口失败为尚无review-context命令及尚无正式模板资源；不是gate已验证。new测试part_22已落盘，含goal原文不变、缺/假/陈旧ack、资源篡改及architecture_stable恢复；APPSDK_TEST_BINARY selector已在测试consumer增加｜w6-blackbox/focused-red.log与part_22.rs｜基线产品代码仍未修改，新测试candidate dirty｜代码接通后绿色行为验收；资源失败留下的本轮fixture须在最终收口按owner确认回收。

2026-10-07 / canonical入口基线｜command -v appsdk = /Users/fanzhang/.cargo/bin/appsdk，version 0.1.0010；cargo在同目录。当前Git无core.hooksPath设置，仅sample hooks；不豁免任何门禁｜当前工具回执｜仅读取，不安装/不重启｜后续正式installer一次加载候选版本并用此consumer固定入口复跑。

2026-10-07 / W5范围拆分｜W5持续版本/迁移/源码扫描且仅起点笔记，无产品diff；为消除真实code+release集中依赖，取消精确自有PID12355/12333，不碰W6或其他worker。成功source观察抽取留存，不算实现完成｜w5-code/events/stderr/source-observations.txt；当前git产品diff为空｜session33380待结束确认；base未变｜W5R只生产context/producer/gate/Rust；W7只分发/版本/maps/迁移，main-resource补丁由parent按交付物集成，避免同写main.rs。

2026-10-07 / 重新并行派单｜W5R session18260负责全部生产Rust和template嵌入；W7 session64113负责JSON/maps/正常0.1.0011版本迁移，额外main资源以patch交parent后集成，不同写main.rs。W6 session40613继续测试owner。旧W5 session33380 exit0仅因TERM结束，不是实现成功｜w5r-context/contract.md、w7-release/contract.md与stdout事件｜源base未变；已审设计无语义降级｜接收三个产物后组合一次公开黑盒，避免辅助迁移研究阻塞主代码。

2026-10-07 / release派单边界补充｜正常新迁移若需Rust SDK_MAP_MIGRATION_STEPS注册，W7可交付exact patch但不能写生产Rust；parent在W5R退出后串行集成该patch。合同追加，未证明W7已读，不把文件写入当作worker消费｜w7-release/contract.md｜partition clarification，无设计语义变更｜报告接收时核对完整Rust版本接线。

2026-10-07 / 疑难生产接线升级｜W5R再次只有源码侦察，无起点笔记/产品diff；取消精确自有PID95380/95347，保留成功观察，不按worker exit0宣称完成。按全局/宿主“疑难攻关先GPT-6.1”新建gcm profile + explicit gpt-6.1-sol实现worker，不用resume/fork。W6/W7范围保留｜w5r-context/events/source-observations.txt；生产gitdiff为空｜独立DESIGN_PASS有效，无实现证据失效｜W5S限生产Rust，不再进行版本或完整项目审计。

2026-10-07 / 分发依赖纠正｜W7无JSON/version产物且仍扫描已取消W5R事件寻找代码符号，构成失效依赖等待。取消精确自有PID95379/95348，保留migration/source观察。新建gcm+gpt-6.1-sol W7S，冻结build_review_context/review_context/assert_review_requirements_binding符号，可独立完成JSON及exact Rust接线patch，无需等生产源码｜w7-release/source-observations.txt/events；当前version/contract无diff｜W5S session17622已写起点，W6继续｜正常版本迁移为当前复杂边界，按疑难先6.1处理。

2026-10-07 / 关键接线owner转交｜W5S仅起点+反复source观察，未有生产diff；取消精确PID79315/79305后，由parent接手已审设计的核心Rust接线。GCM W7S仍拥有分发/迁移，W6仍拥有测试，W3模板/W4独立设计证据复用。此为编排者消除关键路径bottleneck，后续独立架构review不得由parent自审替代｜w5s-context/events/notes；当前产品diff为空｜设计与接口不变，外置候选不变｜parent生产Rust实现，等W7 exact release patch后组合测试。

2026-10-07 / parent核心实现｜生产Rust接线已落盘：唯一review_context owner、共用作者readiness（无下游review依赖）、完整goal/candidate/validation/evidence/template上下文、PASS显式ack先纳入project_bindings再生成identity、modern下游重算绑定、historical frozen维持原语义。模板resource与渲染复用同一嵌入常量；本地index与文件共改仍由模板源比较拒绝｜rust/src/main/review_context.rs及main/review_gates/lifecycle_closure diff；parent-check.log｜code dirty未绑定最终SHA；需W7manifest/版本接线｜编译后组合分发与W6公开黑盒；尚不宣称产品通过。

2026-10-07 / W6传输失败｜session40613 exit1，stream disconnected/decoding response body连续重试5/5后turn.failed；无report/final，不能标完成。已保留新公开黑盒和现代fixture部分diff，首次红测5FAIL有效；不删除部分产物。生产代码已由parent落盘并compile PASS，新fresh GCM worker只接手测试收口，不复用旧session｜w6-blackbox/events/stderr/focused-red.log；tests parts01/04/05/06/22｜模板manifest/版本仍由W7S负责，未组合则不能宣称green｜重新派发测试验证；parent最终组合检查。

2026-10-07 / formatted生产编译｜格式化仅本轮生产Rust，第二次cargo check已完成；历史communication/store_runtime未使用import advisory保留，未扩大修改。新版manifest未组合前不跑consumer绿测｜parent-check-formatted.log；parent-source-registry.log｜生产candidate dirty，W7S拥有maps/version可能后续失效registry证据｜source gate当前检查后等正式bundle组合。

2026-10-07 / source registry FAIL｜SDK_SOURCE_LINE_LIMIT:rust/tests/cli_smoke/part_01.rs:1520>1500。新增GCM测试helper使既有part超限；保留门禁，将helper移至同一module包含的新part_22即可。测试合同追加，未观测worker读取前不称已消费｜parent-source-registry.log；W6R contract｜生产code check PASS；tests owner独占写入｜W6R修正或其报告接收后parent串行接线，不放宽limit。

2026-10-07 / 切片收口｜W7S/W6R仍无产物，准确PID60044/60034/18606/18548已TERM并核对退出。parent已字节复制四份0.1.0010历史maps；新GCM W8 maps/schema、W9 bundle/version、W10 tests各自独占小范围，不等失效worker。parent负责迁移与Rust表接线；取消不等于完成｜原始worker logs保留｜当前编译PASS，行为未绿｜组合后验收。

2026-10-07 / 迁移与旁路检查｜parent新增0.1.0010历史map及新迁移声明（target digest待W8映射收口）、Rust版本迁移steps/manifest/historical表已接线。governance.rs受1500行门禁约束，新历史映射归既有canonical_map owner，未放宽门禁；当前1498行。dagpipe fmt与全targets tests PASS，源码未变证据可复用。fetch确认origin/main仍c3c0c8d｜dagpipe-tests.log、当前diff｜主树新增他人docs/collab.md dirty，禁止清理/覆盖，集成边界需处理｜继续组合GCM产物及公开CLI验收。

2026-10-07 / bundle集成owner｜W9仍仅重复源码分发检查且未起点笔记/修改；准确PID15112/15068已TERM，parent接手版本与manifest并落盘正常0.1.0011。W8仍独占maps/schema、W10测试；未报告者不算完成｜W9原始事件；parent diff｜迁移target digests待W8完成｜下一步公开CLI测试。

2026-10-07 / 组合收口｜W8/W10重复静态扫描且无实质diff，准确PID15110/15062/15109/15067已TERM核对退出，保留原始记录。parent接手最小maps/schema、helper移位、当前version fixtures与恢复下游断言；正常迁移digest已绑定真实maps。原W6的公开红测试产物复用，原W3模板/W4R独立设计仍有效｜当前产品/测试diff；focused-green-attempt1.log｜完整候选dirty，尚无green｜现执行真实CLIconsumer定向验证，失败按唯一owner修复。

2026-10-07 / 同入口首个失败定位｜组合后5例失败均在pin-lock前置：SDK_MIGRATION_LIVE_MAP_UNRECONCILED:resource-map.json，尚未进入context。真实发布owner init.rs仍硬编码0.1.0010，pin_lock未接受新version且最后migration停0010；在唯一owner修为0011及新步骤，保留旧步骤/历史target，未弱化map检查。SDK source registry PASS（测试helper移位，不放宽1500门禁）｜focused-green-attempt1.log、attempt2.log；当前diff｜与真实新CLIconsumer对应｜重跑受影响context测试，后续全Rust回归。

2026-10-07 / 作者验证收口｜全部Rust targets合计519例PASS：full-attempt4已通过40单元、339 CLI、6 Collab schema、99 communication CLI、5 event schema、20 request schema、7 optional governance、1 sdk_0008；最终sdk_0009陈旧版本断言导致该完整命令exit101，修正当前版本fixture后单独2例PASS。不得将完整命令写为exit0。生产修复还覆盖controlled_verified阶段旧PASS拒绝；context生成仍无下游依赖，旧PASS恢复公开链已通过｜rust-full-attempt4.log、sdk-migration-green.log、recovery-attempt4.log｜组合候选dirty，尚未commit；最终fixture修正后无生产变化｜安装同一release后执行canonical消费者。

2026-10-07 / 图注册与release｜manifest两图已嵌入现有dagpipe设计注册，11个operator仅设计占位，execute明确返回DAGPIPE_DESIGN_OPERATOR_NOT_EXECUTABLE，不声称需求Runtime实现。dagpipe全targets PASS、格式/JSON/source registry检查PASS；正式release build exit0｜dagpipe-tests.log、release-build.log、当前source registry回执｜SDK 0.1.0011，base c3c0c8d；尚未安装、实现后review、集成或push｜更新当前证据，官方installer后取得installed行为证据，再绑定候选并独立审查。

2026-10-07T14:06:52Z / 安装与盘恢复｜官方installer exit0；Intel盘短暂掉线并导致cwd不可用，未prune/重建/清理。恢复原盘后已读installer回执，installed/release binary哈希一致，模板cmp一致｜install-global.log、implementation-checkpoint.md及工具回执｜installed0.1.0011，binary SHA256 2d5b64065f1b985cc034043d2016908bf4be2cc9c052c7c5ba70a209de2bb59a｜当前固定入口执行E2E。

2026-10-07 / installed E2E PASS｜固定APPSDK_TEST_BINARY安装路径，5个context场景+1个goal/binding gate+1个stale ack全部PASS，各命令exit0；涵盖原文保持、模板分发、未确认/缺证据拒绝、合法ack、假/缺/陈旧ack、旧PASS拒绝及公开恢复链。context/hash/check不能冒充认证。fetch后origin/main仍c3c0c8d｜installed-review-context.log、installed-review-gate.log、installed-stale-context.log；author-validation.md｜当前生产/模板/maps/tests未再变；未有最终candidate SHA｜归档必要证据，候选source锚定提交后正式独立review。
