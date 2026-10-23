# 工作角色、分工和指定审查

状态：源码实现与隔离公开接口已验证。图校验、安装和用户 live 状态分别计证据，图本身不表示运行已生效。

用户可给 peer 指派 `designer`、`reviewer` 等多个工作角色。角色随稳定名字保存，
不改变 master/peer 的协作职责和授权。master 将用户在任务中提出的自然语言要求
保存为明确的分工字段。daemon 只处理这些字段，不从任务正文猜测分类或分工。

项目规则、目标规则和任务明确字段逐项覆盖。保存到任务后的有效约束不会被后来
修改的默认规则追改。指定执行者是硬约束；工作角色是优先选择条件。指定审查者
须完成该任务的审查，声明 reviewer 角色本身不授予其他任务的审查权。

## 角色更新

本图对应 [角色操作图](../../../docs/dagpipe/collab-work-role-registration.graph.json)。
未提供角色字段表示保留，明确空集合表示清空。用户指派其他 peer 的角色时，
master 使用同一持久化 owner。角色更新不修改通信绑定或 master 授权。

```mermaid
flowchart TD
  S([接收角色请求]):::boundary --> A[解析当前身份与目标名字]:::action
  A --> B[区分协作职责与工作专长]:::action
  B --> C[(按稳定名字保存或读取角色)]:::data
  C --> E([返回角色与明确错误]):::boundary
  classDef boundary fill:#E9EEF5,stroke:#64748B,color:#0F172A
  classDef action fill:#E8F1FF,stroke:#2563EB,color:#0F172A
  classDef data fill:#F1EAFE,stroke:#7C3AED,color:#0F172A
  linkStyle default stroke:#475569
```

## 规则与分派

本图对应 [分派操作图](../../../docs/dagpipe/collab-role-assignment.graph.json)。
图内每一阶段传递成功、等待或拒绝结果。收到失败后，后续阶段不执行写入或通知。
角色偏好无匹配时可选择其他可用 peer，但须保存原因；指定人不可用时只能等待。
邀请已保存与目标已接受是两个事实。接受前不转移执行责任。

```mermaid
flowchart TD
  S([接收分工请求]):::boundary --> A[读取当前任务与分工规则]:::action
  A --> B[合并适用规则并保存任务约束]:::action
  B --> C[按指定人员或角色优先选择]:::action
  C --> D[通过现有邀请接收门禁]:::action
  D --> E([返回指派或等待原因]):::boundary
  classDef boundary fill:#E9EEF5,stroke:#64748B,color:#0F172A
  classDef action fill:#E8F1FF,stroke:#2563EB,color:#0F172A
  linkStyle default stroke:#475569
```

## 指定审查与交付

本图对应 [审查交付操作图](../../../docs/dagpipe/collab-required-review.graph.json)。
`delivered` 表示提交待审，指定 reviewer 接受后才可继续原有集成与验收流程。
代码任务保留合并义务；只读任务继续原有无需合并路径。返工或实质修改任务合同后，
旧审查不能继续证明当前任务通过。进度更新和同名换会话不使审查失效。

```mermaid
flowchart TD
  S([接收交付审查请求]):::boundary --> A[读取任务约束与交付记录]:::action
  A --> B[核对指定审查者与当前审查结果]:::action
  B --> C[提交待审返工或原有交付状态]:::action
  C --> D[派送审查请求并区分回执]:::action
  D --> E([返回交付结果与下一步]):::boundary
  classDef boundary fill:#E9EEF5,stroke:#64748B,color:#0F172A
  classDef action fill:#E8F1FF,stroke:#2563EB,color:#0F172A
  linkStyle default stroke:#475569
```

三个图分别为单源单汇，拓扑无环。等待、拒绝和返工是本次操作的结果；下一次操作
使用新的请求重新进入。所有可修改任务状态的入口均须执行对应约束，包括直接邀请、
认领、普通状态更新和旧分派入口。人工终止、清理完成与验收交付分别投影，不能互相替代。

## 必要验证

- 公开 CLI 注册、master 指派角色、省略保留、明确清空与稳定名字恢复。
- 角色偏好、无匹配原因、指定人不可用、继承与覆盖、邀请接收前后的归属。
- 指定执行者不能被认领、直接邀请或旧入口绕过。
- 指定 reviewer 可审查，其他人不能代审；返工、合同变更和再交付使旧结果失效。
- 原有代码集成、只读交付和人工终止语义；journal replay 与 compaction 保留新约束。

安装和用户 daemon 生效需要另外的证据。源码与隔离测试不证明当前全局 daemon 已加载新实现。
