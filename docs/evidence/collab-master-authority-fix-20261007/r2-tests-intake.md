# Final test intake

The parent received the GCM fixture edits from the isolated r2-tests worktree.
The worker was stopped at a completed read-only tool boundary. Session 32947
exited 0 after interruption; this is not a worker DONE receipt.

Eleven owned test files were copied into the combined candidate. The parent
compared timers.rs, collab-mcp.rs and the final part_07.rs against the worker
input; all three matched byte-for-byte. Production owners were preserved.

The parent corrected the unfinished public recovery fixture. It now obtains
the server/session/pane PID tuple from the target pane, uses the pane returned
by new-session, authenticates re-registration with the existing runtime context,
and reads the recovered principal through the public Context request. It does
not use a fresh bootstrap request with unrelated session/thread anchors to
assert identity continuity. The corrected test passed, exit 0, one actual test.

The parent added a public multi-route and concurrent-mutation test. It registers
two app scopes in one project and another project on distinct live pane
addresses. Status, Context and BoardShow report their exact scopes and holders.
Concurrent approved clear/promote requests produce complete observations.
Clearing one route leaves the other two assigned. This test passed, exit 0.

Full combined binary and public consumer validation is running. These targeted
results do not claim installed, architecture-review or main delivery completion.

The first serial binary run had 924 passes and one failure in the pending
reservation fixture. Its concurrent poll/recovery can select native notification
before consumption; the intentionally read-only audit then rejects that path.
An immediate isolated rerun passed without changes, confirming nondeterministic
fixture ordering. The test now completes poll before recovery to specify the
already-consumed pending-reservation scenario. Original payload, task, admission
and single-audit assertions remain. No production notification logic changed.

The earlier owned DSH bootstrap probe was attributed to this worker's exact
mktemp command and its printed `/tmp/dshprobe.uFrw` receipt. Darwin libproc
reported PID 41753 cwd under that root. Canonical `collab down` with that probe's
state directory returned exit 0. PID 41753 was absent, its socket was released,
and the owned root was removed with an absence check. The formal daemon and
unrelated probe were untouched.
