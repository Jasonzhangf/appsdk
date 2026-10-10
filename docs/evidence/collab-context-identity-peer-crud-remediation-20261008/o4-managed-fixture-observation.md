# O4 — F1 managed fixture support observation

Task: `collab-context-identity-peer-crud-remediation-20261008`  
Base: `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a` plus the task's uncommitted design artifacts.  
Method: read-only source and test-fixture inspection. No daemon, identity, route, grant, subscription, peer, journal, mailbox, or runtime state was changed. No fixture was executed; runtime reachability remains `UNVERIFIED`.

## Typed result

- Production/public managed-record creation: `UNSUPPORTED` on this base.
- Real CLI/MCP consumer driving managed `Send`/`Ready` with an isolated AppServer RPC fixture: `UNSUPPORTED` on this base. The CLI does not dispatch these mutations, and no supported path creates/binds the required managed record.
- Public readback of message ID, message state, wake attempts/transport evidence, `consumed_by_recv`, and task state: `SUPPORTED` through existing status commands.
- Public durable readback of `repair_required`, escalation/failure class, and notification result: `UNVERIFIED`; those fields exist in action response data that the current subagent path discards.
- No supported public managed fixture was found in inspected sources. Private state seeding is not public lifecycle evidence.

## Evidence

`collab/src/main.rs:356-371` handles `Start` by returning an unsupported error, sends only `List` and `Status` through `Req::SubagentObserve`, and returns `Ok(())` for other actions. `collab/src/main_context.rs:358-365` classifies only those two queries. No production CLI code constructs `Req::Subagent`; its server handler exists at `collab/src/server/mod_parts/part_11.rs:41-52`, but the public CLI never reaches it. The MCP wrapper maps arguments and treats exit status as success (`collab/src/bin/collab-mcp.rs:179-189,217-257`), so empty stdout becomes `isError=false` with empty content.

The daemon also rejects `Start` (`collab/src/subagent.rs:846-855`). Production `Dispatch` reuses an existing idle managed child and returns `MANAGED_SUBAGENT_UNSUPPORTED` when none exists (`collab/src/server/mod_parts/part_07.rs:907-931,958-969`). The candidate list reads existing `state.subagents` (`part_07.rs:324-380`). The only child launch helper is `#[cfg(test)]` (`collab/src/subagent.rs:563-801`); non-test `SubagentUpdated` producers update or re-snapshot existing records. There is no production path to create the managed record and binding that `Send`/`Ready` require.

## Fixture classification

| Seam | Evidence | Classification |
|---|---|---|
| `AppFixture` Unix-socket AppServer RPC and isolated CLI environment | `collab/tests/appserver_two_tui_integration.rs:105-171,343-398` | Public consumer setup plus supported protocol fixture for ordinary peers |
| Real CLI + MCP stdio and owned tmux fixture | `collab/tests/mcp_master_authority_cli.rs:42-158` | Public consumer setup; does not create managed child |
| `register_private_dispatch_peer` seeds `Event::SubagentUpdated` | `collab/src/server/scheduler_admission_tests/part_01.rs:16-24` | Direct private journal/state mutation |
| `subagent_tests.rs` seeds `SubagentUpdated` | `collab/src/subagent_tests.rs:267,339` | Direct private state mutation |
| `register_appserver_worker`, `test_server`, `test_appserver_transport` | `collab/src/server/peer_tests/part_07.rs:1208`; `peer_tests/part_01.rs:569,622` | `cfg(test)` helper |
| Journal-fault injection | `collab/src/server/mod_parts/part_01.rs:400-456` | `cfg(test)` in-process fault injection |
| Public fixture that creates/binds a managed peer | Not found | `UNVERIFIED`; no evidence of a supported path |

The ordinary-peer AppServer integration proves only that the protocol fixture can accept an ordinary `collab send` (`appserver_two_tui_integration.rs:549-587`). It does not make managed Send/Ready reachable.

## Readback and remaining projection gap

- `collab msg <id>` returns message identity, state, wake attempts/transport evidence, and consumption receipt (`collab/src/main.rs:930-935`; `server/mod_parts/part_11.rs:301-330`).
- `collab subagent status <id>` reads managed status (`main_context.rs:358-365`); `collab task status <id>` reads task state (`main.rs:1116`; `part_11.rs:432-443`).
- `repair_required`, failure/escalation, and notification outcome are produced in response data (`server/mod_parts/part_05.rs:1077-1133`), but `subagent::notify` discards failed response data (`subagent.rs:412-437`). `Ready` also ignores successful repair data (`subagent.rs:1018-1027`). These have no public durable readback today.

## Branch reachability

| Branch | Current evidence/reachability |
|---|---|
| Notification accepted | Ordinary peer only (`appserver_two_tui_integration.rs:574-587`); managed path unreachable |
| Notification rejected | RPC fixture has an error branch (`appserver_two_tui_integration.rs:380-397`); managed path unreachable |
| Notification not attempted | Disabled-notifications path exists (`server/mod_parts/part_05.rs:892-894`; `config.rs:306-310`) but managed consumer path is unreachable |
| Missing subscription | Response owner exists (`part_05.rs:1083-1090`); managed consumer path is unreachable |
| Ready reused | Owner exists (`subagent.rs:933-935`); requires managed record |
| Post-commit journal failure | Only test injector is available (`server/mod_parts/part_01.rs:400-421`) |

## B3–B5

| Case | Result | Evidence |
|---|---|---|
| B3 Send durable commit + notification failure | `UNSUPPORTED` | CLI no-op; no managed fixture; notify discards response data |
| B4 Ready commit + notification failure | `UNSUPPORTED` | CLI no-op; no managed fixture; Ready discards repair projection |
| B5 Ready with no subscription | `UNSUPPORTED` | Response owner exists, but CLI and managed fixture cannot reach it |

The previous design review's two P1 findings remain valid: `me()` can commit identity reconciliation before the requested action, and `notify`/`Ready` lose response data. O4 adds a more upstream F1 break and the lack of a supported managed fixture. This observation does not grant implementation admission, lower A1–A12, or replace the separate A6 peer-creation requirement.
