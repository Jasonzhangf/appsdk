# Goal Clarification Contract

Goal clarification is the first gate for development and debug. It precedes issue admission, claims, Playground work, red tests, and formal source changes.

```text
User request
  -> parse objective
  -> restate understanding
  -> acceptance criteria
  -> non-goals and assumptions
  -> ambiguities and focused questions
  -> user confirmation
  -> claim/admission
  -> Playground or implementation
```

`GoalClarificationRecord` stores the raw request, understood objective, acceptance criteria, non-goals, assumptions, ambiguities, questions/answers, scope, risk decisions, confirmation identity/time, and lifecycle status.

`confirmed` means the user accepted the restatement and all material questions are answered. `admitted` additionally means feature/module owner, allowed paths, and required gates are bound.

For bugs, this contract defines expected behavior and non-goals first; evidence-first debugging still requires baseline reproduction, first divergence, positive/negative intervention, and unique-owner evidence in Playground.

No claim, Playground mutation, red test, source patch, compile, promotion, or issue close is valid while the goal is `received`, `parsed`, or `clarification_pending`.
