# App Server immediate-notification verification evidence

Base: `448ace19b0e949f1d8a1919c259e46f6219dbd3c`
Code candidate: `e3b34e2fc96cd4913c96a00942c7665bb6bf22bd`
Code tree: `d6da3ea284df7ce94be9dc2fac7ef804cbd7199a`
Environment: macOS arm64, Codex CLI `0.154.0`, Collab `0.2.0048`

The isolated checks below ran against the exact code candidate tree above.
This evidence document is committed in a docs-only child of that candidate;
the code tree is unchanged by that child.

## Isolation

- Started a disposable `codex app-server` with a temporary `CODEX_HOME` and
  `unix://` socket under `/tmp/collab-9cb568f-integration.hjFaBC`.
- Created two real persisted App Server threads through `thread/start`.
- Used no existing `~/.collab` route, daemon, socket, device, or terminal
  session.
- Stopped only the disposable App Server PID with `SIGTERM` and waited for it
  to exit.

Observed isolated identities:

```text
thread_a=01a0c0ef-a610-7b61-9aca-194d2c57f67b
thread_b=01a0c0ef-a656-76f2-a291-a59344fe438e
```

## Loaded admission

Command:

```sh
COLLAB_APPSERVER_SOCKET=<isolated.sock> \
COLLAB_APPSERVER_NAMESPACE=codex_app \
CODEX_THREAD_ID=<thread_a> \
cargo test --manifest-path collab/Cargo.toml \
  client::adapters::codex_app_server::tests::live_appserver_candidate_is_admitted_when_loaded_or_rejected_when_unloaded \
  -- --nocapture
```

Result: `1 passed, 0 failed`; the real loaded thread passed admission.

## Immediate notification

Command:

```sh
COLLAB_APPSERVER_SOCKET=<isolated.sock> \
COLLAB_APPSERVER_NAMESPACE=codex_app \
CODEX_THREAD_ID=<thread_b> \
cargo test --manifest-path collab/Cargo.toml \
  client::adapters::codex_app_server::tests::live_immediate_notify_accepts_loaded_thread \
  -- --ignored --nocapture
```

Result: `1 passed, 0 failed`; the real App Server accepted `turn/start` for
the loaded thread.

## Focused source checks

```sh
cargo fmt --manifest-path collab/Cargo.toml -- --check
cargo test --manifest-path collab/Cargo.toml notification -- --nocapture
cargo test --manifest-path collab/Cargo.toml active_turn_selection -- --nocapture
git diff --check
```

Result: formatting and diff checks passed; the notification suite reported
`34 passed, 0 failed`, and the active-turn selection suite reported
`7 passed, 0 failed`.
