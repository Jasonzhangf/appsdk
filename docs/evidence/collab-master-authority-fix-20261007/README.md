# Collab master authority fix: evidence

Task: `collab-master-authority-fix-20261007`. Base: `7350fbf6b020b1464d531337c7b2f6b8fa5de6f2`. Defect: `1f79966`.

The user requested a simplified master state machine, a GCM implementation worker, main integration, rebuild and restart. The author owns an isolated candidate worktree. The parent owns installation, formal runtime validation, independent architecture review and integration.

Authoritative user instructions, received in the parent Codex conversation on 2026-10-07:

> 我们现在colab有重大的问题 即使我重新初始化 APP SDK 我也无法清除错误的旧的 master 状态 我们的 panel 对于 master 是否是活着的 对于 Tmux  这种情况下面判断是错的 而且他不给覆盖 这是他妈极其愚蠢的 你画一下当前的 DAG 图，找到我们现在的错误的地方，然后告诉我：
> 1. 我们在这个设计上有哪些是错误的？
> 2. 哪些是需要消融的？
>
> 整个状态机需要重新画，重新简化。

The subsequent implementation instruction was:

> 派gcm worker解决问题 合并到main，重建重启

The accepted design adds explicit approval-based clear and keeps initialization separate from authority mutation. The user did not request a scope migration, deletion of tasks/messages, automatic master promotion, a panel write endpoint or weaker recipient validation. No later instruction changed the product requirements. Updated global execution instructions govern planning, verification and delivery.

The initial observation is a frozen pre-plan record. Its child-sandbox `DAEMON_UNKNOWN` observation was corrected by a parent invocation: the installed daemon answered `collab status --all` successfully. This correction does not change the observed tmux authority defect. The DSH CLI bootstrap gap is separate: its identity-context candidate is absent; the affected DSH consumer uses the daemon's existing public registration boundary.

Current evidence:

- `plan.md`: accepted independent OAuth / gpt-6.1-sol plan, READY.
- `design-review.md`: independent pre-code design review, PASS. This is not an implementation or architecture PASS.
- `baseline-bin.log`: base binary test suite, 925 passed, 1 ignored, exit 0.
- `baseline-native-registration.log`: installed 0.2.0253 isolated native first registration, PASS. Its temporary root was removed. This proves baseline entry capability, not the fix.
- `graph-consumer.log`: 15 actual graph consumer tests passed, exit 0. An earlier zero-test filtered invocation is excluded.
- `p01-tmux-base-red.log`: base public CLI/daemon/tmux regression, cargo exit 101; 1 passed, 2 failed. The holder status fails with unknown-liveness authority rejection; clear is an unrecognized command. The wrapper printed `EXIT=101`; its final echo exit 0 is not the cargo result.
- `p01-appserver-base.log`: public AppServer replacement control, 1 passed, cargo exit 0. This unaffected baseline path is a regression control.
- `p02-tmux-first-green.log`: first development green on the same three tmux public consumer tests, cargo exit 0. This is an intermediate author observation; final candidate and installed validation remain pending.
- `p02-diagnosis.md`: independent OAuth / gpt-6.1-sol read-only diagnosis of the 39 partial-regression failures. This is a concrete fixture/source diagnosis, not review PASS.
- `r2-docs-result.md`: completed repository Skill/contract ablation and three graph validations. Parent also corrected the old force-close instruction to match the existing authorization rule: unreachable Master does not permit owner bypass.
- `r2-source-intake.md`: sealed GCM production-code handoff and receipt limits. The source worker was stopped at a completed tool boundary without a DONE receipt; source completion depends on the combined tests. Named `r2-source-*-incremental.log` files are intermediate smoke evidence; `r2-source-bin-failed.log` still has37 failures and is not acceptance.

The parent received and combined the GCM production and fixture edits. All GCM
writers are stopped. `r2-tests-intake.md` records the interrupted handoff and the
parent's completion of public recovery, scope and concurrency fixtures.

- `author-bins.log`: exact combined production input, 925 Collab tests passed,
  1 ignored; 18 MCP tests passed. Cargo session 44673 exited 0. Serial execution
  avoids the prior parallel tmux fixture-query failures. The earlier failed
  runs are retained in the external task records and are not acceptance.
- `author-public.log`: AppServer 15, DSH 1, master-status 2, MCP 1 and tmux 13
  public tests passed, cargo session 87630 exit 0. Each consumer uses its own
  project/state/socket. These tests exercise actual CLI or public wire entry.
- `author-dsh-unreachable.log`: the final DSH test additionally stops its owned
  gateway, verifies the assigned holder remains and transport is not live,
  clears through an authenticated peer, and verifies restart preservation.
  Cargo session 59028 exited 0; this supersedes the earlier DSH-only case.
- `recovery-public.log`: same-principal public registration advances the binding
  generation and reissues the grant; a new principal does not inherit it; the
  old generation clear request is rejected without changing the holder.
- `scope-concurrency-public.log`: two app scopes in one project and another
  project report exact scope/holder; concurrent clear/promote reads are complete;
  clear affects only its route.

The source baseline remains origin/main 7350fbf6 at the 2026-10-08 09:38 UTC
action boundary. `candidate-runtime.md` records official installation 0.2.0256,
formal down/up, canonical PID/path, public replay 32/32 and real native context
PASS. It also records the explicit separate Desktop control-socket failure.
`architecture-review.json` records the independent AGY controller PASS, exit 0,
no findings, and three concrete module boundary records for reviewed tree
515b53132888fe7e7670eabfccee0936ba1ad672. Only evidence receipts were added after
that review; production source, tests and configuration are unchanged.
PR #16 merged as `bb9ce30b0efe60ab48445fcf4b3efd9c13c40fc4` after both CI runs
passed. `main-runtime.md` records the main rebuild, official down/up and installed
32-test replay. The separate Desktop control-socket failure remains explicit.

`source-registry-correction.md` records the first PR release gate failure and
the test-only move into two targets with one shared fixture. The original
baseline registry evidence was invalid for the added 2000-line test file.
The current staged source registry passes. Debug and installed tmux consumers
both passed 7+6 tests. Product source/Skill inputs remain unchanged.
`test-layout-review.json` records independent AGY PASS, exit 0, no P0/P1, for
the test move at tree e0f546ad2f56a6cca5c54a3670ea0d11e6eead0b. Its P2 warning
about unused shared fixture items is retained as advisory; no post-review test
change was made. The first review remains the product-change receipt.

Archived logs retain the actual result text. Only trailing whitespace and blank
EOF lines were normalized in earlier repository copies for git diff --check.
Final main and cleanup logs are archived losslessly as gzip files. Necessary raw
result logs and the canonical note remain in external task records. Isolated
worker homes were removed. No test result was rewritten.

Raw agent event streams, reasoning, credential files and unrelated runtime state are not included.
