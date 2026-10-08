# Public DSH wire and MCP consumer handoff

Source-only observation: 2026-10-08 07:44:14 UTC. Read worktree HEAD/base `7350fbf6b020b1464d531337c7b2f6b8fa5de6f2` plus task-owned dirty production, including parent’s 07:31:07 UTC final projection intake. This is an implementation blueprint for only NEW `collab/tests/dsh_master_authority_cli.rs` and `collab/tests/mcp_master_authority_cli.rs`. No process, test, registration, or inference was run. Source bindings are in `notes.md`.

**1. Isolated daemon and exact wire admission**

Reuse the small local primitives from `tests/tmux_recv_e2e.rs:14–180`: unique short temporary root, `root/h` state, `.agent-collab` baseline directory, command wrapper and owned cleanup. CLI binary = `COLLAB_TEST_BINARY`, otherwise `env!("CARGO_BIN_EXE_collab")`. Every command uses fixture cwd, `COLLAB_STATE_DIR=<root/h>`, `CODEX_HOME=<root/home>`. Remove inherited `COLLAB_APPSERVER_SOCKET`, `CODEX_APP_SERVER_SOCKET`, `COLLAB_APPSERVER_NAMESPACE`, `TMUX`, `TMUX_PANE`, `CODEX_SESSION_ID`, `CODEX_THREAD_ID`, `COLLAB_WORKER`; also remove `DSH_SESSION_ID` for isolated non-DSH CLI callers. Tmux callers set `CODEX_INTERNAL_ORIGINATOR_OVERRIDE=Codex TUI` as the existing fixture does.

For DSH, run selected CLI `up` without a pane. Its own executable launches `serve` (`client.rs:319–346`, `main.rs:423–451`). Use `<root/h>/server.sock`; wait with bounded `Ping` requests. Use fixture `down`, wait for socket removal, then `up` for restart. Keep the gateway alive. Never touch the shared daemon or delete identities/journals for this restart test.

Collab socket framing is one JSON object followed by LF, with one JSON response line. `RequestEnvelope` flattens the operation; there is no nested `request`, `params`, or JSON-RPC header (`proto.rs:703`, `client.rs:191`). Canonicalize root first. For this fixture choose `<APP>` from the declared CLI namespace `identity.rs:71`, `CLI_APP_SERVER_ID = "appserver-cli"`. It permits the generic read-only CLI commands below to address the same scope. This is a source-defined fixture choice, not discovery of an installed app ID. Both peers use the same app/root.

First registration, with no runtime_context:

```json
{"op":"Register","worker_id":"<A>","token":"<TOKEN_A>","cwd":"<CANON_ROOT>","candidates":{"dsh":{"endpoint":"unix://<ABS_GATEWAY_SOCKET>","runtime_id":"<GATEWAY_RUNTIME>","agent_id":"<AGENT_A>","session_id":"<AGENT_A>","cwd":"<CANON_ROOT>"}},"project_context":{"app_scope_id":"<APP>","canonical_root":"<CANON_ROOT>","project_scope":"<CANON_ROOT>"}}
```

Register B with distinct worker/token/agent IDs, same root/app/gateway runtime. Parse raw response `ok`; failure is `ok:false,error:<string>`, success fields are top-level, not under `data`. Require `typed:true`, expected `worker_id`, `transport_selected.kind:"dsh"`. Binding is **`command.binding`**; `command.cmd` is `"RegisterWorker"` (`state.rs:86`, `part_06.rs:749`, `identity.rs:248–309`). Check binding agent/root/app, positive generation, session/native-thread matching the candidate. Build `<CTX_A>` from these returned fields:

```json
{"app_scope_id":"<RETURNED_APP>","canonical_root":"<CANON_ROOT>","project_scope":"<RETURNED_PROJECT_SCOPE>","runtime_context":{"agent_id":"<RETURNED_AGENT>","runtime_id":"<RETURNED_RUNTIME>","appserver_id":"<RETURNED_APP>","endpoint_generation":1,"binding_id":"<RETURNED_BINDING>","session_id":"<RETURNED_SESSION>","native_thread_id":"<RETURNED_THREAD>"}}
```

Here `1` illustrates first registration; copy the actual generation. Do not confuse gateway candidate runtime with returned collab runtime. Admission compares the token, unique current binding, root/app, generation and session/thread (`part_10.rs:818–890`). Each authority request is the following literal operation object **plus** top-level `project_context:<CTX_A>`; use `<CTX_B>` when B acts:

```json
{"op":"MasterPromote","worker_id":"<A>","token":"<TOKEN_A>","approval":"fixture user approved promotion"}
{"op":"MasterDelegate","worker_id":"<A>","token":"<TOKEN_A>","target_id":"<B>"}
{"op":"MasterClear","worker_id":"<B>","token":"<TOKEN_B>","approval":"fixture user approved clear"}
{"op":"MasterStatus"}
```

Status only needs root/app context. These three mutations have no `command` field. Promote returns `master:<A>,mode:"user_approved_self_promotion"`; delegate returns `master:<B>,delegated_by:<A>`; clear returns `master:null,previous_worker_id,was_empty,mode:"user_approved_clear"`. Status returns `master:null` or an object with `worker_id,binding_id,endpoint_generation,assigned_by,approval`; `recorded_unusable:null`. Every successful authority receipt/status has top-level `scope:{project_scope,app_scope_id}`, including Empty (`part_07.rs:1170–1312`, `part_08.rs:6–116`). Compare against registration, never infer from the holder.

**2. DSH gateway callback: Unix NDJSON, no HTTP routes**

Copy only `Gateway::start`, its bounded owned teardown, and the idea of `facts_reply` from `server/dsh_channel_tests.rs:26–166` into the new integration file. Do not copy `test_server`, `register_dsh`, or private handlers. The former is an external gateway fixture; the latter bypass public admission.

The actual endpoint is `unix://<absolute control.sock>`. There are **no HTTP methods, URL paths, or HTTP bodies** in this adapter (`adapters/dsh.rs:109–275`). Each accepted connection reads one NDJSON request, calls the response callback, writes one LF-terminated response, then closes:

```json
{"method":"agent-facts","params":{"nonce":"<FRESH_32_HEX>","runtimeId":"<GATEWAY_RUNTIME>","agentId":"<AGENT_A_OR_B>"}}
{"ok":true,"result":{"nonce":"<EXACT_REQUEST_NONCE>","runtimeId":"<REQUEST_RUNTIME>","agentId":"<REQUEST_AGENT>","sessionId":"<REQUEST_AGENT>","cwd":"<CANON_ROOT>","status":"running"}}
```

The nonce is minted by collab on every challenge. Echo it exactly; do not precompute it. Respond for both registered agents and repeated presence/route probes. Reject unknown identities with `{"ok":false,"error":{"code":"unknown-agent","message":"fixture unknown agent"}}`. Registration checks runtime/agent/session equality, canonical cwd equality, and nonempty status (`part_03.rs:129–196`). `probe` uses the same `agent-facts` exchange; no separate probe route exists. The response above suffices for registration and status.

If the preservation test sends a message, handle `method:"enqueue"` too. Its params are `runtimeId,agentId,mode,content:[{type:"text",text}],sender:{runtimeId:"collab",id,name:"collab"},messageId`. Return `{"ok":true,"result":{"messageId":"<REQUEST_MESSAGE_ID>","runtimeId":"<REQUEST_RUNTIME>","agentId":"<REQUEST_AGENT>"}}` (`dsh.rs:328–355`). This proves queue admission only. No model execution, consumption or ACK follows from it.

**3. Real MCP subprocess paired with tmux CLI**

Use `single_pane_fixture` from `tmux_recv_e2e.rs:155`: `tmux -S <owned socket> new-session -d -s <label> 'sleep 600'`; obtain server PID and pane ID with `display-message -p '#{pid}'` and `'#{pane_id}'`. Set `TMUX=<socket>,<server_pid>,0`, `TMUX_PANE=<pane>`, distinct fixture `CODEX_SESSION_ID`/`CODEX_THREAD_ID`, and the isolated env above. Run actual `collab context`; take worker ID from `identity.worker_id`, scope/binding from returned context. CLI `master promote --approval <text>` establishes Assigned.

Select MCP executable exactly as parent `r3-public/task.md`: `COLLAB_TEST_MCP_BINARY` if set; otherwise sibling `collab-mcp` of explicitly selected `COLLAB_TEST_BINARY`; otherwise `env!("CARGO_BIN_EXE_collab-mcp")`. An absent installed sibling is a capability error, not permission to mix debug MCP with installed CLI. Spawn it with piped stdin/stdout, fixture cwd/env, and **`COLLAB_BIN=<selected CLI>`**. Product `collab_bin()` reads `COLLAB_BIN`; it does not read `COLLAB_TEST_BINARY` (`collab-mcp.rs:169–196`).

Use the supported line framing; flush and read each response by matching ID:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"collab_master","arguments":{"action":"clear","approval":"fixture user approved clear"}}}
```

Notifications produce no reply. Initialization returns matching protocol version, tools capability and `serverInfo.name:"collab"`. Actual clear requires matching ID, no JSON-RPC error, `result.isError:false`, `result.content[0].type:"text"`; parse its `text` as CLI JSON and assert clear fields/scope. Then assert actual CLI `master status` is Empty in the same scope. Empty approval `""` must return `result.isError:true` with text containing `master clear requires explicit user approval`; it is a tool error, not a top-level JSON-RPC error (`collab-mcp.rs:412–439,551–604`). Missing approval instead fails argument construction, so it does not test daemon approval admission. Close child stdin to deliver EOF, drain stdout/stderr, and wait for exit; no MCP shutdown method is implemented. Cleanup fixture daemon and only its tmux server/socket.

**4. Implement the smallest main path, then extend it**

1. DSH: start gateway/daemon; register A/B; assert initial Empty and exact scope. Promote A. Send B’s clear with `approval:""`; assert `ok:false` and approval error, then status still names A with the same binding/generation/assignment metadata.
2. Clear as authenticated **nonmaster B** with valid approval. Assert previous A, `was_empty:false`, Empty status and retained scope. Clear again: `was_empty:true`, previous null, same scope. MCP: implement the analogous empty-approval, valid-clear and idempotent sequence through actual `tools/call` first.
3. DSH: promote A, delegate A→B, assert delegate receipt/status; seed business data; clear as A while B holds authority; restart isolated daemon; read status and public records without re-registering or editing state. No resurrection; both peers and data remain.
4. On an Assigned slot, add wrong-token and changed-generation refusals with before/after status comparisons. Change only the tested request field. Report actual error text; do not weaken authentication to get a successful clear.

Public observations: `collab master status`; `collab who` → `count,workers`; `collab status --all` → `summary.workers/messages/tasks`, `workers`, `tasks`; `collab task status <id>`; `collab mailbox read --all`; authenticated tmux `collab msg <id>` → `id,body,consumed_by_recv`. DSH equivalents are wire `Workers`, `StatusAll`, `TaskStatus` with `task_id`, `MailboxRead` with `all:true`, and `MsgStatus` with `msg_id`, carrying the fixture scope. CLI `who/status --all/mailbox --all` use the declared CLI app scope, hence the explicit choice in §1.

Seed tmux data via existing `task register <id> --next 'survive clear'` and `sendmessage --to <worker> --subject 'preservation' 'preserve me'` (`tmux_recv_e2e.rs:1011–1036`). DSH task seed: authenticated wire `TaskRegister` with `task_id,priority:"p2",next_step:"survive clear"`. DSH send seed: authenticated `Send` with `from:A,worker_id:A,token,to:B,type:"notify",subject,body,delivery:"queued"`; supply required `command:{command_id,operation_id,actor_binding_id:<returned>,endpoint_generation:<returned>,scope:{app_scope_id,project_scope_id}}` and A context (`part_08.rs:163–221`). IDs are fixture-owned unique strings. Check stable task/message IDs/body and peer counts; compare `consumed_by_recv` without issuing recv/ACK or asserting wake completion. Authority and optional transport observations must not be conflated.

**5. Capability limit and stop boundary**

Fresh DSH CLI bootstrap is unsupported: `server/identity_context.rs:95–105` constructs `dsh:None`; env collection reads only `DSH_SESSION_ID`, not gateway address/runtime/agent facts (`adapters/codex_app_server_production_part1.rs:380–389`). Existing provisional CLI rebind explicitly rejects a DSH candidate (`part_10.rs:777–789`). Use the public wire Register above; do not add product bootstrap or invent DSH env knobs. `master_status_cli.rs` hand-writes routes and supplies a fake daemon, so reuse its framing knowledge only. No blocker for the selected wire/MCP boundaries was demonstrated by this source read. Installed binary pairing and actual runtime behavior remain unverified. Parent’s final-source marker is open; next action is implementing the two owned files and capturing actual test exits, not further schema exploration.
