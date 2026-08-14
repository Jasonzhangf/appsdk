# AppSDK Goal Prompt Generation Plan

## Goal

把用户给出的自然语言目标转换为可直接执行的短 `/goal` 提示词，并绑定一份完整实现计划、项目范围、验证门槛和完成标准。

## Acceptance criteria

- 输出固定结构：目标、实现文档、执行规范、验证、完成标准。
- 生成前先创建或确认 `docs/goals/<feature-name>-plan.md`。
- Prompt 只引用实现文档，不复制长篇实现细节。
- Prompt 明确“本任务不再生成新的 prompt”。
- 未确认的目标不能进入实现、debug、Playground 或正式测试。
- 目标确认记录能回链到 issue、scope、owner 和 required gates。

## In scope

- Goal prompt 生成规范。
- Goal clarification 与 `/goal` 的衔接。
- AppSDK Skill 使用说明。
- 项目文档和模板入口。

## Out of scope

- 自动替用户回答业务歧义。
- 跳过用户确认直接修改代码。
- 把实现计划全文塞入 prompt。
- 用 prompt 替代 Evidence、Review、Promotion、Freeze records。

## Design

```text
raw user goal
  -> clarify objective and ambiguity
  -> user confirms
  -> write implementation plan
  -> emit compact /goal prompt
  -> execute through Playground/review/promotion/freeze
```

`/goal` 是执行入口，不是第二套治理状态机。GoalClarificationRecord 是目标真源；实现计划是方案真源；`/goal` 只提供可复制的执行指针。

## Files

- `docs/goals/<feature-name>-plan.md`: per-goal implementation plan;
- `docs/design/goal-clarification-contract.md`: goal confirmation contract;
- `skills/appsdk-project-governance/SKILL.md`: project governance workflow;
- template `.appsdk/goal.json`: project goal record.

## Risks

- Prompt 过长导致执行者忽略硬门槛；通过只保留摘要和文档引用规避。
- 用户目标仍有灰色区间；通过 confirmed/admitted 门禁阻止执行。
- Prompt 与实现文档漂移；通过每次生成前重新确认计划路径和验收标准规避。

## Verification plan

- Skill format validation。
- 文档路径和模板入口检查。
- Goal clarification positive/negative gate tests。
- SDK CLI verify/compile gate。
- 最终 architecture review。

## Implementation steps

1. 保留 GoalClarificationRecord 作为目标真源。
2. 为每个具体目标创建实现计划文档。
3. 生成固定五段式 `/goal` prompt。
4. 按 prompt 执行，不递归生成新 prompt。
5. 用 tests/build/review/evidence 收口。

## Definition of done

- 用户给出目标后，可得到一份短 `/goal` prompt。
- Prompt 能指向完整实现计划。
- Prompt 不绕过目标确认和 AppSDK 生命周期门禁。
- 执行结果有可验证证据，不能只凭文本声明完成。
