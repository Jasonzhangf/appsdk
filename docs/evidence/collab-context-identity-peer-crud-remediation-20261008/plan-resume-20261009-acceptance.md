# Fresh-plan acceptance — 2026-10-09

Task: `collab-context-identity-peer-crud-remediation-20261008`
Planner: fresh Codex `oauth` profile with explicit `gpt-6.1-sol`; thread `01a11f15-181c-7e01-a9e1-4224da8f5897`; CLI session `95906`; wrapper PID `5607`; Codex PID `5611`; process exit `0`; final event `turn.completed`.
Plan: [`plan-resume-20261009.md`](plan-resume-20261009.md)
SHA-256: `e8ce1907a6b63eb5fb9dea4cca1177cd6da214dec4bf6939fe2e5be3b5e3f5a4`
Input: [`latest-observation-20261009.md`](/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/latest-observation-20261009.md), candidate HEAD `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`. `git ls-remote origin refs/heads/main` returned the same SHA during acceptance.

## Decision

Accept the plan as `READY` for the bounded D2/D3 design sequence only. It preserves all four user capabilities, F1–F4, A1–A12, installed/live delivery, implementation review, Git integration, and cleanup. It does not authorize product implementation, Create, daemon changes, user identity operations, installation, or runtime mutation. A6 remains `BLOCKED` under O5-R4.

## Accepted next action

Run **D2-A** only: map host-local approved identity recovery admission to the existing durable receipt owner and restart-safe public query, and identify the exact missing producers/reducers/replay/query surfaces. The deliverable is `admission-receipt-map.md`, node notes, and a result in the new D2-A run directory. This is read-only and admits only D2-B contract design after parent review. Do not start D2-B, D3-RUC, B23, F2/F3 product code, or any Create implementation until the D2-A mapping is received and accepted.

The execution contract requires early phase evidence, parent-owned PID/session polling, review after an observed idle interval, and controlled interruption only when the specific live handle has first been checked and its work/side effects are understood. The interrupted D2/D3 workers' raw evidence remains preserved and is not accepted as a design result.

## Controller refinement

The planner's P0 table includes updating `W/note.md`. The active task's sole task truth is the Goal file plus its evidence `run-notes.md`, and the required correction is already recorded in those files. No change to project `note.md` is needed for this plan acceptance; avoid a duplicate task summary. This does not change the accepted D2-A scope or the project's separate memory ownership.

## Evidence limits

- The prior D1-R4 review PASS applies only to D1/F1 design.
- F1 tests remain local logic/compile evidence; daemon-backed public CLI/MCP remains unverified due the recorded AF_UNIX/loopback `EPERM`.
- The one live but `unknown` Collab peer endpoint is not verified as a task dispatch channel; the lost worker is unavailable. Do not bootstrap or promote this Goal executor.
- No product edits or tests were performed by the planner. The Goal remains `active / INCOMPLETE`.
