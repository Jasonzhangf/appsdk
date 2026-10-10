# Collab 四项能力审计（2026-10-08）

结论：当前不满足本次四项要求。普通 context 引导已有 daemon 主链；用户批准的身份覆盖恢复、master peer CRUD 和 context 操作指引仍未闭合。安装版另有 subagent 写操作空成功。

本轮只审计产品，不修改实现或 Skill，不注册身份，不变更 master，不安装或重启 daemon。新增本报告及公开入口观测回执。

## 基线

- 源码 HEAD：`2cac9e935944bddcb98fb6e7af4beb95966dcff9`。
- 安装 CLI/MCP：`/Users/fanzhang/.cargo/bin/collab`、`collab-mcp`，均报告 `0.2.0258`；摘要见 `public-entry-observations.json`。
- 源 Skill 与全局 Skill 的 SHA-256 一致，都是 1257 行。源文件为 `collab/skills/collab/SKILL.md`；全局文件为 `/Users/fanzhang/.agents/skills/collab/SKILL.md`。
- 主树已有修改：`docs/collab.md` 及其他任务的未跟踪证据。全部保留。该文档按当前工作副本审计，不当成已提交内容。
- 源码证据与安装版观测分别记录；未独立建立安装字节与 HEAD 的完整构建来源等价性。

## 四项对照

| 要求 | 当前结果 | 判定 |
|---|---|---|
| 一次 context 完成身份确认、恢复、注册；缺资料时明确补交 | CLI 收集事实；daemon 选择/恢复身份、Register、持久化 binding，再返回 Context。缺资料返回 required_fields/action；补交入口是 context --provide。CLI 仍自行裁定补交事实冲突；冲突等错误没有完整可执行补救描述。 | 部分符合；成功恢复未做本轮 live 验证 |
| master/peer 都可按用户批准覆盖并一键恢复身份 | master promote/clear 只处理 grant。promote 先调用 me，必须先解决身份；context/provide 不接受 approval 或目标身份。身份冲突仍直接失败。 | 不符合 |
| master 创建、关闭 peer，具备 CRUD；context 介绍命令 | 没有 peer 创建入口；subagent start 显式不支持。worker close 有入口但依赖 snapshot，而 snapshot 始终不支持。subagent 多个写动作未派发就退出 0。context 没有 CRUD 操作卡。 | 不符合 |
| Skill 明确 context 能力、触发条件与操作，agent 不用猜 | 普通 bootstrap/缺资料流程有说明。但仍宣传未接通的 subagent 写路径；禁止 approval supplement，与本次批准恢复要求冲突。多个章节重复入口；缺失字段只给描述，没有实际来源/取值方法。 | 部分说明已有；整体不符合 |

## 发现

### F1 [P1] subagent 写入口返回空成功，未到 daemon

位置：`collab/src/main.rs:356-371`；`collab/src/main_context.rs:358`。

Start 显式报不支持。List/Status 构造 SubagentObserve。其余 action 返回 None，CLI 直接 Ok(())，没有身份鉴权、Req::Subagent 派发或回执。因此 Dispatch/Send/Ready/Working/Rearm/Close/Snapshot 在根解析成功后均走空成功。

本轮安装版公开入口复核：

- `collab subagent close audit-nonexistent-01a11e24`：退出 0，stdout/stderr 为空。
- MCP `collab_subagent(action=close, id=audit-nonexistent-01a11e24)`：`isError=false`，内容为空。
- MCP 仅根据 CLI 退出状态判定成功：`collab/src/bin/collab-mcp.rs:179-189`。

最小处理：在唯一 CLI owner 接通已支持的 typed 请求与真实回执；未支持动作显式报错，CLI/MCP/Skill 同步能力。不能仅把空结果改成成功 JSON。

### F2 [P1] 用户批准无法解决身份覆盖恢复

位置：`collab/src/main_context.rs:136-146,196-223`；`collab/src/main.rs:654-660`；`collab/src/server/identity_context.rs:25-28`。

--provide 只接受 session_id/thread_id/endpoint/namespace，且客户端拒绝与自动观察值冲突。没有批准裁决字段、目标身份或后台批准恢复请求。

master promote 会先 me()，me 又走同一 IdentityContext；如果此时身份冲突或 credential 失败，就到不了批准 grant 更新。因此“已认证 peer 替换 master 授权”不能替代“用户批准恢复 peer/master 身份”。

源码中 master grant 更新不查询 incumbent/candidate liveness，这部分符合批准替换授权的方向；但前置注册/绑定要求仍存在。锚点设计文档 `docs/design/collab-anchor-restore-model.md:68-75` 明确只允许 master 归属裁决，普通 peer 不走批准裁决。

本轮安装版：--provide 内 approval/worker_id 都被拒绝；context --approval 被参数解析拒绝。

最小方向：扩展现有 context typed 请求，由 daemon 接收、核验并执行本项目内的批准身份裁决；CLI 只收集并提交输入。保留 identity/binding 与 master grant 各自的唯一 owner。禁止自动猜目标、复制 token 或用手改文件恢复。

### F3 [P1] peer 创建缺失，关闭流程依赖无法产出的快照

位置：`collab/src/subagent.rs:846-856`；`collab/src/server/mod_parts/part_07.rs:1317-1428`。

- 没有 ordinary peer 创建命令；managed subagent start 在 CLI 和 daemon 都明确拒绝。
- `worker close <id> --reason <text>` 已接通 daemon，要求当前 master、目标没有未完成任务，并持有匹配当前 AppServer thread 的快照。
- `handle_worker_snapshot` 验证后始终返回 WORKER_SNAPSHOT_UNSUPPORTED，包括 AppServer 目标；无法生成新的关闭前置快照。
- tmux peer 没有绑定 AppServer thread 时，close 更早拒绝；新注册会清除旧快照（`state_impl.rs:262-264`），所以不能把历史快照当成新注册 peer 的常规关闭路径。
- daemon 的 subagent close 即使走通，也只标记记录 closed，明确保留 peer 注册与 route（`subagent.rs:1136-1150`）；它不等于关闭 peer/runtime。

最小方向：先定义创建/更新/关闭 ordinary peer 的实际宿主与 owner；修通正式 peer 生命周期。按真实任务、消息、资源责任决定关闭前置条件。若快照是必需契约就修复生产者；若不是当前关闭契约所需就消融该死前置。不得伪造快照或借 subagent record-only close 宣称 peer 关闭成功。

### F4 [P2] context 操作清单与 Skill 不能让 agent 直接按事实操作

位置：`collab/src/server/mod_parts/part_09.rs:1293-1333`；`collab/skills/collab/SKILL.md:518-578,1050-1098`。

operations 主要复制 role_brief 的职责/下一步文字，另有 pending merge 说明和 master 为空时的 promote 提示。没有 master peer CRUD 的命令、参数、前置条件、批准要求及失败后的准确动作；已分配 master 的替换/clear 也没有完整操作提示。

Skill 说明了只跑 context、按 required_fields 补 --provide，却仍要求使用当前 CLI 空成功的 dispatch/send/close/ready/working 等路径。说明分散在 Recovery、command card、Situation、状态表、Worktree identity，且批准恢复被明确禁止。

缺字段返回 field_descriptions/action，但描述只是“Current runtime ...”，没有对应宿主的事实来源/获取方式。显式冲突经 Resp::err 返回，而 Skill 部分表述又要求从 requires_identity_update 读取 exact_error；缺字段构造本身也未提供 exact_error。agent 不能假定每种错误都有同一补交对象。

最小方向：复用现有 operations，按当前角色和已确认能力返回 command、必要参数、when、requires_approval、前置条件与预期回执。缺资料返回真实来源及提交格式；需批准返回明确批准动作；不可执行返回具体阻断。Skill 主文只保留一张“状态 → 动作”表，其他章节引用它。修项目源 Skill 后通过正式安装刷新全局副本。

## 推荐顺序与验收

1. 优先修 F1，消除 CLI/MCP 空成功；在公开入口断言不存在目标、越权、失败及真实状态回读。
2. 在现有 context owner 内补批准身份裁决。成功、缺事实、需批准、明确失败都给准确动作；普通自动恢复保持一次调用。
3. 闭合 master peer 创建/更新/关闭，验证 runtime、注册、binding/route 与责任处理的真实结果。不要让“记录 closed”冒充完整关闭。
4. 最后统一 context 操作卡、MCP catalog、源 Skill 与全局安装 Skill，删除重复及退役说明。

上述为审计建议，尚未进入独立开发计划或实施。

## 本轮证据与未验证项

已验证：源码调用链、安装版 help/version、补交字段拒绝、start 不支持、CLI/MCP 不存在 child 的空成功、源/全局 Skill 摘要相同。

未验证：生产身份创建/恢复的成功行为、生产 master grant 替换、真实 peer 创建/关闭、daemon live 版本与源码等价性。本轮未跑作者全套测试或做独立实现 review；这些不作为本次只读审计结论的 PASS 依据。

原始公开入口结果：`public-entry-observations.json`。只新增审计文件，保留主树已有修改与他人资源。
