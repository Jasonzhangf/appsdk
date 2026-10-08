# Installed candidate

Base: 7350fbf6b020b1464d531337c7b2f6b8fa5de6f2. Combined uncommitted
candidate in the task-owned external worktree. No production source changed
after final author validation. Installation session 96954 exited 0.

Official `scripts/install-global-collab.sh` built and installed 0.2.0256. Its
receipt proves candidate/installed binary equality and all seven embedded Skill
files match the repository sources. Installed digests:

- CLI: 4d5acbad69228d36516b02636a109a8a846d4681636820803eca346bf9a32ba5
- MCP: 993dd28ca3b8b7eb00009c0dd41d62e363f05acbc822746199a98db4f9d521ef

At the runtime boundary, formal server.pid was 50423. Darwin libproc reported
that PID's cwd as the RouteCodex project. Canonical `collab down` from that cwd
exited 0; PID 50423 disappeared and the formal socket was released. Canonical
`collab up` from the same cwd exited 0. New PID 14298 owns the formal server.pid;
proc_pidpath reports `/Users/fanzhang/.cargo/bin/collab`.

Public before/after comparison preserved both projects' durable task fields,
peer IDs and active task IDs, message counts, and master worker/approval/
assigned_by/assigned_ms/endpoint_generation. The task status projection's
keepalive.pending_since_ms changed during restart; it is an observation field,
and is explicitly excluded from durable task comparison. Neither project's
formal authority was cleared, replaced or reset by this task.

`candidate-installed-public.log` uses COLLAB_TEST_BINARY and
COLLAB_TEST_MCP_BINARY set to the canonical installed paths. Session 31016 exited
0: AppServer 15, DSH 1, status 2, MCP 1, tmux 13, total 32 tests passed. This
includes the final unreachable DSH gateway case and the real MCP initialize,
notifications/initialized and tools/call exchange.

`candidate-installed-native.log` is the existing native registration script,
with COLLAB_BIN set to canonical collab. It exited 0 and reports
isolated_appserver_first_registration=PASS. It starts a real AppServer thread
without a model turn or production message. Its `/tmp/cg.e1McgU` root was removed;
the parent verified absence.

The current Desktop environment's direct formal `collab context` returned exit
1: APPSERVER_ENDPOINT_REJECTED / ADAPTER_ROUTE_UNAVAILABLE because the separate
`~/.codex/app-server-control/app-server-control.sock` refuses connections. This
is preserved in `formal-context.stderr`, not treated as a successful identity
recovery. No identity facts were invented and no non-target service restarted.
Installed native context capability is proven by the isolated real AppServer
consumer above. Existing Desktop MCP processes were not claimed to hot reload;
the fresh installed MCP process is the verified consumer.

Independent architecture review and main delivery remain pending.
