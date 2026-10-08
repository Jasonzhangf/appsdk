# Source registry correction

PR #16 candidate 7edbab1d failed the release job's
`appsdk verify-sdk-source-registry .` gate. The same public gate reproduced
locally: SDK_SOURCE_LINE_LIMIT:collab/tests/tmux_recv_e2e.rs:2000>1500.
The baseline registry PASS did not cover the later added test lines. It must
not be reused as final candidate evidence.

The parent moved the seven Master authority tests into
`collab/tests/tmux_master_authority_cli.rs` and placed the one shared fixture in
`collab/tests/support/tmux_cli_fixture.rs`, included by both targets. The old
target retains its six original receive/context tests. The resulting files
have 725, 1037 and 240 lines. No source limit or registry owner was changed.
Reassembled nonblank helper/test lines match the previous test file exactly
and in order; only file boundaries and include declarations changed.

Both targets passed against debug binaries (13 tests, session 19731 exit 0)
and canonical installed 0.2.0256 (13 tests, session 72225 exit 0). rustfmt check
and cached diff check passed. The public source registry gate passed after
the three files were staged, including the new shared helper.

Production src and embedded Skill subtrees remain respectively
19466713267b075cb9fcc0be774c25a0d42692e8 and
5f806ef2cfbeb4b7d6341fc7fc17b5a1a352275a, identical to the first reviewed,
installed candidate. The existing 925+18 binary tests, other 19 public tests,
formal down/up, digest equality and real native context evidence remain valid.
The affected tmux consumers have new actual installed replay above. No extra
formal restart was needed for this test-only move. A fresh independent review
of the changed candidate is required before another commit/push.
