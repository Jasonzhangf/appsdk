# Exact candidate consumption

Primary consumed author exit 0, turn.completed and stopped PID7588. Current
production/test/fixture hashes equal the author receipt; the current public CLI
suite has 34 pin tests plus the separately executed witness test, covering all
five mapped writer regressions. Build, fmt, strict source registry, registry10
and static pin-history graph passed on base9a9d674.

Author compared its production against the old standalone transition candidate
and therefore declined reuse. Primary instead verified all six production
fingerprints against the already accepted two-unit composition in
`docs/evidence/1a21b0c-transition-impl-20261003/composed-archive-primary-receipt.json`.
Its migration hash3b29d6d8 and the other five hashes exactly match this candidate.
That unchanged full-archive pin/verify/reentry and history-immutability evidence
is reused, not reported as freshly executed or as installed acceptance. Existing
red-before-fix evidence remains historical causal proof. Current registry and
public tests are fresh; source-layout dependency has independently delivered.

The retained executable was moved out of Git evidence without changing bytes:
`/Users/fanzhang/.codex/task-evidence/agentteams/receipts/1a21b0c-transition-final-20261004/retained/appsdk`, SHA-256
`c17d46e38f29b7153d534fc221132989be4e00a64d2ce5acc35a1c173b7bdbd1`.
Original author's relative binary path describes its earlier retained location.
Full raw author evidence and the consumption/reuse receipt are under
`/Users/fanzhang/.codex/task-evidence/agentteams/receipts/1a21b0c-transition-final-20261004`.

No independent architecture verdict, commit, integration, remote push,
canonical installation or Teams admission is claimed at this stage.
