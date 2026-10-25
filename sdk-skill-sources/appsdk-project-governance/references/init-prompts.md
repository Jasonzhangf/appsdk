# AppSDK 初始化与协作入口

AppSDK 项目初始化、Collab 身份注册和本人目标调度是不同操作。只执行当前请求需要的入口，
不把它们组成每次开发前的固定步骤。项目治理沿
[bootstrap-migration](bootstrap-migration.md)；协作沿已安装的
[Collab Skill](../../collab/SKILL.md)。

## 注册和恢复

只有用户明确要求注册或已有身份确需恢复时，在当前项目使用：

```sh
collab context
```

daemon 选择身份和通信绑定，调用者只提供当前宿主的真实能力事实。稳定名字承载工作，
agent sessionID 是执行载体；tmux pane/session、thread、socket 不替代工作归属。
Desktop、TUI 和 DSH 的具体事实与收件差异沿 Collab Skill，不在本文件维护第二套宿主表。

复用回执中的当前身份、角色、项目 peer/master 与本人责任；成功后直接继续。
只有返回 required_fields 时，按对应 reason/action 补充真实缺项。不要手填旧 worker、token、
route，或在成功回执后再固定调用 context、who、status。普通身份恢复不要求收发回环；
显式通信维修或通信功能验收才验证受影响通路。

首次注册失败时保留原错，继续不依赖协作的工作；既有身份通信失败不撤销已领任务。
无关任务不停止。身份/route 由 Collab 管理，用户管理全局 daemon，agent 不启动、关闭、
重启它或终止 tmux server/宿主 pane。具体恢复见
[stable-name-recovery](../../collab/references/stable-name-recovery.md)。

## master、peer 和目标

注册不授予 master 权限。用户已有的明确授权不重复询问；当前名字接管或 master 变更
仍沿 Collab 的当前授权合同。角色操作返回的新 brief 足够时，不再重复读取完整 context。

master 与 peer 都编排自己的目标，区别是目标层级。长程子目标交获授权且接受的 peer；
一次性执行/观察沿当前宿主编排入口。公开 board、模板、指定执行者/reviewer 与资源
生命周期只在 Collab Skill 维护，不在初始化提示里复制整套派单规则。

本人已接受长程目标且需要调度时，复用已有目标文件与已约定周期：

```sh
appsdk goal subscribe --goal <existing-goal.md> --interval <agreed-period>
appsdk goal status --json
```

任务/笔记/普通 timer 不是目标注册。保留真实 owner 与注册结果，master 不替 peer 持有目标。
当前有效回执已包含所需状态时直接复用；只有存在具体缺口才补 status。不臆定 `10m`
等周期，不把缺失巡检服务变成 ready 工作的阻塞，也不在普通目标登记时要求短周期 live replay。
需要验证调度功能的变更才检查 armed、触发、消费与一次性重武装。目标结束或无可执行工作时
结束本人有限调度，保留基础直达消息接收；详见
[notifications](../../collab/references/notifications.md#patrol-duties-and-finite-schedules)。

## SDK 缺陷报告

本项目问题沿已有 canonical intake 去重。消费者遇到 SDK 本身缺陷时，可使用明确的上游入口：

```sh
appsdk bug list -q "<symptom>" --json --upstream
appsdk bug new --upstream -t "[SDK Bug] <symptom>" -m "<expected; observed; reproduction; evidence>" -l "<actual-priority>,appsdk"
```

已有相同问题时补充其证据，不重复创建。报告最小复现、预期/实际、首个失败层和相关证据位置；
版本信息仅在判定源码/安装差异时补充。不要固定把所有问题标为 P0，不要求报完再 show 全文。
给对应 owner 可行动信息即可，不粘贴完整日志或新增候选摘要包装。登记不是修复或验收完成。

## 显式重置

普通恢复、规则升级和报错不触发重置。用户明确授权丢弃特定状态时，分别使用所属入口：
AppSDK 治理重置沿 [bootstrap-migration](bootstrap-migration.md)，Collab 项目重置由当前
master 沿 [migration-daemon](../../collab/references/migration-daemon.md#reset-is-explicit-discard-not-recovery) 执行。
两者不互相删除状态，不通过手删 journal、socket、lock 或第二 daemon 绕过失败。
