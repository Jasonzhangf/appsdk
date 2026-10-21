# Goal Clarification Contract

This contract applies when a project has explicitly enabled the
`GoalClarificationRecord` admission workflow. Ordinary development and debug
follow the host workflow; this record is not a second universal entry gate.

```text
User request
  -> parse objective
  -> restate understanding
  -> acceptance criteria
  -> non-goals and assumptions
  -> ambiguities and focused questions
  -> reuse existing explicit authorization or resolve material unanswered questions
  -> claim/admission
  -> Playground or implementation
```

`GoalClarificationRecord` stores the raw request, understood objective, acceptance criteria, non-goals, assumptions, ambiguities, questions/answers, scope, risk decisions, confirmation identity/time, and lifecycle status.

`confirmed` means the goal and scope are explicitly authorized and all material
questions are answered. Existing user instructions can supply that authorization;
do not require another restatement/confirmation round for the same accepted goal.
`admitted` additionally means feature/module owner, allowed paths, and required
gates are bound. Record the real authorization, not an invented approval.

For bugs, this contract defines expected behavior and non-goals first; evidence-first debugging still requires baseline reproduction, first divergence, positive/negative intervention, and unique-owner evidence in Playground.

Within this opted-in admission workflow, unresolved material questions stop only
their dependent claim or mutation. Continue independent authorized work. A
pending record does not add a project-wide stop or require routine promotion or
freeze. Record state alone does not replace behavior evidence or host plan/review.
