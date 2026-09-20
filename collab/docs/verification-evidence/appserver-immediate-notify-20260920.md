# App Server immediate-notification verification evidence

Base: `0f3005cbcb4aa4ca4bd6a6ad2427367d72727630`
Candidate: `8b9c06a624a804d4bab2849ae937492d2bf6b5d1`
Environment: macOS arm64, Codex CLI `0.154.0`, Collab `0.2.0043`

The review scope binds this evidence to the exact candidate commit and tree.

## Isolation

- Started a disposable `codex app-server` with a temporary `CODEX_HOME` and
  `unix://` socket under `/tmp`.
- Created two real persisted App Server threads through `thread/start`.
- Used no existing `~/.collab` route, daemon, socket, device, or terminal
  session.
- Stopped only the disposable App Server PID with `SIGTERM` and waited for it
  to exit.

Observed isolated identities:

```text
thread_a=01a0c0ce-09a7-74d2-8d20-4a09f8126cc6
thread_b=01a0c0ce-0a4e-7412-a9d9-4018073b9a90
```

## Loaded admission

Command:

```sh
COLLAB_APPSERVER_SOCKET=<isolated.sock> \
COLLAB_APPSERVER_NAMESPACE=codex_app \
CODEX_THREAD_ID=<thread_a> \
cargo test --manifest-path collab/Cargo.toml \
  client::adapters::codex_app_server::tests::live_appserver_candidate_is_admitted_when_loaded_or_startable \
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
