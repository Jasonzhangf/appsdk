# b160ada static graph registry repair

Base: `bb9168d86342c7cfe910e1124b25e6bb7a8cffd9`

| Time (America/Los_Angeles) | Node | Status | Input / version | Evidence | Next |
|---|---|---|---|---|---|
| 2026-10-03 05:00 | baseline | confirmed | clean worktree `codex/b160ada-registry-20261003` at `bb9168d` | `git status`, `git rev-parse HEAD` captured in session | validate existing design graphs before edits |
| 2026-10-03 05:07 | static graph validation | pass | both graph files at `bb9168d` | `docs/evidence/b160ada-registry-20261003/raw-graph-validation.txt` | build public CLI and capture red |
| 2026-10-03 05:09 | red | confirmed | `rust/src/dagpipe.rs` at `bb9168d`; CLI build in task-owned target | `docs/evidence/b160ada-registry-20261003/raw-red-cli.txt` | patch typed registrations and public CLI assertion |
| 2026-10-03 05:10 | implementation | complete | `rust/src/dagpipe.rs`, `rust/tests/cli_smoke/part_17.rs`; unchanged design graphs | working diff | run focused format/test/CLI verification |
| 2026-10-03 05:11 | focused CLI test | pass | post-fix source at working tree; isolated target/home/tmp | `dagpipe_validate_reports_every_embedded_graph_as_single_source_single_sink`: 1 passed | run focused `dagpipe_fix_` tests and candidate CLI comparisons |
| 2026-10-03 05:13 | candidate CLI comparison | pass | built CLI; source cwd and outside cwd; manifest at `8419cb04...` | `raw-green-cli-source.txt`, `raw-green-cli-outside.txt`: `match=true`, count 9 | run remaining static checks |
| 2026-10-03 05:14 | focused `dagpipe_fix_` tests | pass | post-fix source; isolated target/home/tmp | `raw-green-tests.txt`: 7 passed, 0 failed | final diff/scope/cleanup checks |
| 2026-10-03 05:15 | registry rejection unit test attempt | failed | command used `--lib`, but package `appsdk` has no library target | exact error: `no library targets found in package appsdk` | rerun the same focused filter against the actual binary test target |
| 2026-10-03 05:16 | registry rejection unit tests | pass | post-fix source; actual binary test target | `raw-green-tests.txt`: 2 passed, 0 failed | final diff/scope/cleanup checks |
| 2026-10-03 05:17 | scope and cleanup | pass | final worktree; task-owned `.b160ada-owned` removed | `git status`: only allowed source/test paths modified plus evidence; `test ! -e .b160ada-owned` passed | hand off candidate evidence to parent |
