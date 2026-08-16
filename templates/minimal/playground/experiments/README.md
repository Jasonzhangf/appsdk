# Experiments

Each issue gets an experiment identity and an isolated clean worktree. Record baseline reproduction and first divergence before the formal fix. Bind the committed candidate by commit/tree/diff hash, then record architecture review and post-review effectiveness separately.

Do not merge when reproduction is absent, review inputs are stale, candidate source changed after review, effectiveness replay did not reuse baseline inputs, or merge ancestry cannot be proven. Archive evidence and remove temporary experiment state only after the merged source has compiled, published, frozen, and verified.
