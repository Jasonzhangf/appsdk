# Goal Prompt Generation

When Jason gives a development or debug objective, clarify it before execution:

1. Restate the objective in one sentence.
2. List acceptance criteria.
3. List non-goals and assumptions.
4. Identify ambiguity and ask only material questions.
5. Wait for confirmation when scope, risk, permission, or irreversible behavior is unclear.
6. Write `docs/goals/<feature-name>-plan.md` before emitting the prompt.

Use this compact output:

```text
/goal
目标：<one-sentence objective>

说明：本任务不需要再写新的提示词，直接按实现文档执行。

实现文档：
docs/goals/<feature-name>-plan.md

执行规范：
- 先查项目合同、owner、scope 和真源。
- 只在允许路径修改；禁止 fallback、silent strip、旁路和无关改动。
- 目标未 confirmed/admitted 时停止实现。

验证：
- 运行定向测试、build/compile、verify 和要求的 review gate。
- 无证据不宣称完成。

完成标准：
- 实现计划中的验收标准全部满足。
- 记录、artifact、scope 和 review 结果一致。
```

The prompt is the final execution task. Do not create another prompt for the same task.
