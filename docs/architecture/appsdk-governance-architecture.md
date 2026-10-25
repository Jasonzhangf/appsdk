# AppSDK 治理架构

AppSDK 提供项目合同、确定性编译、验证及可选流程辅助。消费者拥有业务语义和验收。
仓库架构与模块入口以根级 [AGENTS.md](../../AGENTS.md) 为准；日常操作与选测以
[appsdk-dev](../../.agents/skills/appsdk-dev/SKILL.md) 为准。本文件不再定义另一套开发生命周期。

## 唯一归属

| 对象 | owner 与边界 |
|---|---|
| SDK 源码 | `rust/`、`dagpipe/`、`collab/` 各自 package；不是消费者生成物 |
| 机器合同与分发 | `contracts/`、`templates/minimal/`、`sdk-skill-sources/`；正式 installer 分发 |
| 消费者项目合同 | 项目 `.appsdk/`；SDK 不覆盖项目需求、业务源码或用户维护的规则 |
| 本地任务状态 | `.appsdk-control/`；由对应命令写入，不作为第二份项目合同 |
| Guidance | 已声明规则、计划与步骤记录的投影；默认辅助，不调用模型、不生产验收证据 |
| Memory | 历史知识与检索；不改变当前权限、需求、规则或任务状态 |
| AppSDK communication / registry | 各自声明的 runtime、地址及回执；不授予 Collab 身份 |
| Collab | 稳定名字、角色、任务、mailbox、投递和消费状态；不拥有业务 pipeline 或 tmux 生命周期 |
| DAGPipe | 通用图编译与执行机制；项目定义 Operator、输入输出和业务结果 |

生成物和查询结果是投影。它们不能成为可写的第二份真源。跨模块调用走所属公开入口，
不通过日志、消息正文或手改状态文件迁移控制权。

## 日常工作与发布边界

普通开发按实际变更检查 owner、受影响 DAG、失败模式和公开入口行为；复用有效证据。
重大 milestone 的 Review、合流与资源收口沿宿主交付合同。Guidance、Memory、Collab
的可用性不增加独立工作的准入步骤；启用协作后只暂停真实依赖失败能力的工作。

Active/Protected、promotion、FreezeRecord 和历史锁记录只服务已选择且实际需要的
消费者编译或发布合同，不能作为每次开发、设计、审查或交付的默认路线。
日常工作不计算候选 SHA/checksum、不冻结快照、不重建补丁或创建身份包装清单。
若现有程序仍强制旧门禁，报告对应 owner 和原错并修实现，不伪造记录或把文档修改当作修复。

正式编译/发布及历史记录能力见 [项目集成](../design/appsdk-project-integration.md)、
[promotion](../design/playground-active-promotion.md) 与
[生命周期记录](../design/lifecycle-records.md)。已发布历史保持不可变。

## 反馈与交付

每个公开操作只交付当前调用者判断结果、继续工作或恢复失败所需的信息。
保留真实状态、必要关联标识、当前责任和可操作错误；内部事件流水、历史记录和诊断明细按需读取。
写入已接受、投递已接受、已消费、审查通过和最终交付分别报告，不互相替代。

一次交接提供可访问产物、验收结论、关键证据引用、剩余问题和下一负责人。
不默认附全部日志或重复主计划，不为汇报再执行查询、测试或确认。持久日志仍按项目需求记录，
减少默认输出不等于删除历史、完整消息正文或未结责任。

## 验证边界

每个外部对象流有明确输入和结果出口。复用项目现有单源单汇图，只有实际变更图时校验。
DAGPipe CLI 静态通过只证明图结构；可执行 Operator 与行为由 SDK compile 和真实 consumer 验证。
源码、构建、安装、运行行为、远端交付和资源清理各自需要对应证据。

消费者业务规则留在消费者项目。AppSDK 不吸收 RouteCodex 等项目的 provider、协议或业务状态机。
