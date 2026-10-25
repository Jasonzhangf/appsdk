# AppSDK additions to goal prompts

Use the host's [goal-prompt Skill](../../goal-prompt/SKILL.md) for the reusable
prompt structure, role routing, planning, verification and delivery contract.
This reference owns only AppSDK-specific additions; do not maintain a second
generic `/goal` template here. An ordinary development request does not require
generating another prompt or a new implementation document.

Bind the existing project requirements, affected DAG, unique owner, allowed
paths and acceptance evidence. Reuse an existing goal/plan document; create
`docs/goals/<feature>-plan.md` only when the requested goal needs one. Do not
add token, time, context, cost, round or concurrency budgets.

Registered master and peer are both long-horizon orchestrators. Each restores
or registers, updates, rearms and closes its own goal with the current AppSDK
goal entry. A peer does not ask master to register its goal; master must not
proxy-register or take ownership of it. Desktop/TUI placement is irrelevant to
that responsibility. Report the real goal receipt; task/board/note/timer state
alone does not prove registration. Use the [Collab contract](../../collab/SKILL.md#目标编排与接续)
for ownership, finite wake and patrol rules.

Patrol is a safety-net wake, never an execution dependency. Continue ready
authorized work immediately after subscribing or rearming. The peer retains
sole delivery ownership for its scope. Route questions and scoped blockers through
the Collab Skill's clarification rule; help does not transfer ownership. Continue
unaffected work while a decision is pending. Do not close unfinished tasks to stop reminders.

For an authorized migration, reference the [migration Skill](../../appsdk-migration/SKILL.md)
and existing evidence. A prompt grants no new role, destructive authority or
daemon lifecycle permission. Keep source, installed and loaded-runtime evidence
separate. Close owned schedules when complete or idle with no actionable work,
and retain direct-message reception for the next assignment.
