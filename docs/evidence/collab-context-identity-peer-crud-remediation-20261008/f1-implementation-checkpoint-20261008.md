# F1 implementation checkpoint

Date: 2026-10-08 America/Los_Angeles
Candidate: `/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008`
Branch: `codex/collab-context-identity-peer-crud-20261008`
Base: `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`

## Ownership transfer

I1-R4 (`impl-public-entry-v4`, node PID 57668, Codex PID 57673) completed
source observation, added `collab/tests/subagent_public_entry_cli.rs`, and
confirmed the daemon-backed fixture cannot start in this execution sandbox.
At transfer, no product source file had been changed. The parent stopped only
the exact two owned worker PIDs, retained the test target and append-only run
events, then took the existing F1 allowlist as the sole writer.

## Parent changes

- `main.rs`: mutation actions now pass through the existing daemon identity
  context and `Req::Subagent`; Start still rejects before identity
  coordination, and List/Status still use the read-only observe request.
- `subagent.rs`: daemon `Resp` data is preserved for typed notification
  failures; Send returns partial action state and durable message facts;
  Ready returns the notification/repair projection and records reused state.
- A source review found Send's notification-failure response merged the prior
  `working → idle` commit and the follow-up error-record commit into one field.
  It now reports the pre-notification `state_commit` and
  `error_state_commit` separately, and reads the actual current status when
  reporting the partial result.
- `collab-mcp.rs`: successful `collab_subagent` output must be non-empty JSON;
  empty and malformed success output become explicit MCP errors.
- `subagent_tests.rs` and `subagent_public_entry_cli.rs`: add partial response,
  Start refusal, MCP empty/invalid result, and daemon-backed public consumer
  cases.

## Verification

Passed:

- `cargo test --manifest-path collab/Cargo.toml --locked --test subagent_public_entry_cli --no-run`
- `cargo test --manifest-path collab/Cargo.toml --locked --bin collab subagent::tests -- --test-threads=1` — 10/10 passed, including the added response projection test.
- `cargo test --manifest-path collab/Cargo.toml --locked --test subagent_public_entry_cli --no-run` — public consumer target compiles after the Send partial-result correction.
- `cargo test --manifest-path collab/Cargo.toml --locked --bin collab subagent_action_routing_sends_only_list_and_status_to_observe -- --test-threads=1` — 1/1 passed; covers all current Actions and verifies only List/Status use the unauthenticated observe-query route.
- `cargo test --manifest-path collab/Cargo.toml --locked --bin collab notification_failure_keeps_daemon_data_and_partial_action_stage -- --test-threads=1` — 1 passed.
- `cargo test --manifest-path collab/Cargo.toml --locked --test subagent_public_entry_cli start_is_rejected_before_identity_coordination -- --test-threads=1` — 1 passed.
- `cargo test --manifest-path collab/Cargo.toml --locked --test subagent_public_entry_cli mcp_rejects_empty_or_invalid_success_output_for_subagent_only -- --test-threads=1` — 1 passed.
- `git diff --check`.
- `rustfmt --edition 2021 --check collab/src/main_tests_part2.rs`.

The repository-wide `cargo fmt --check` is not clean on the existing baseline;
it reports formatting changes across many unrelated files. No whole-tree
formatting pass was applied. The newly added public test file was formatted.

Blocked, not passed:

- The full `subagent_public_entry_cli` target's List/Status and mutation cases
  require a local daemon Unix socket. Socket binding returns `EPERM` in the
  current sandbox. The direct isolated `collab context` attempt returned
  `DAEMON_UNAVAILABLE`; an existing daemon-backed MCP test also failed before
  its behavior assertion. The earlier `red-subagent-public-entry-cli.log`
  preserves this environment failure.
- No runtime install, daemon restart, managed Create, live replay, or
  architecture review occurred. F1 managed success/partial/repair black-box
  acceptance and overall A9 remain incomplete pending A6 and a socket-capable
  execution environment.

## Remaining F1 checks

1. Complete changed F1 source and test review for action authorization,
   response shape, and no accidental scope expansion.
2. Rerun the complete F1 consumer target where AF_UNIX bind is permitted.
3. Keep public managed success, partial, repair, restart-readback, install, and
   live behavior marked `UNVERIFIED` until their prerequisites and evidence
   exist.
