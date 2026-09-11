# Goal Prompt Generation

Use only when the user asks for a goal prompt. An ordinary development/debug
request does not require generating a prompt or writing a separate plan file.
For a requested prompt, clarify material unknowns:

1. Restate the objective in one sentence.
2. List acceptance criteria.
3. List non-goals and assumptions.
4. Identify ambiguity and ask only material questions.
5. Wait for confirmation when scope, risk, permission, or irreversible behavior is unclear.
6. Write `docs/goals/<feature-name>-plan.md` before emitting the prompt.

For an MVP→M1 migration or closeout, the plan is the single implementation
source and must additionally bind:

- the current MVP baseline and the intended M1 target;
- the owning endpoint/project scope, allowed and forbidden paths, and the
  authorized master/worker boundaries;
- the legacy control-plane inventory, retained evidence, chosen preserve/reset
  route, and exact destructive authorization (if any);
- the five Loop parts (`Trigger`, `Work`, `Gate`, `State`, `Stop`) and the
  per-round order `Discover → Hand off → Verify → Persist → Schedule`;
- the AppSDK source-repository versus managed-project boundary, Codex TUI
  identity and route evidence, two-way replay, and explicit failure/unknown
  handling;
- exact candidate, merge, install, daemon-restart, and deployed replay gates.

Use the [AppSDK migration Skill](../../appsdk-migration/SKILL.md) for the complete
inspect/snapshot/migrate-or-reset/rebind/restart/verify procedure. Do not copy
that state machine into the prompt or into this reference. A prompt cannot
register a goal or grant a role: Desktop never runs `appsdk goal subscribe`;
only the authorized live TUI/master endpoint may register the existing plan.
If the plan is absent, unconfirmed, or not admitted, stop before emitting a
usable execution prompt. A periodic interval is a scheduling choice, not a
ten-second liveness probe or permission to retry a blocked command.

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
