# App Server loaded-admission verification evidence

Base: `456c76fd95d576722614f5f0f165cfe5b878b4e8`
Environment: macOS arm64, Codex CLI `0.154.0`, Cargo `1.97.1`

The review scope binds this evidence to the exact candidate commit and tree.

## Isolation

- Started a disposable `codex app-server` with a temporary `CODEX_HOME` and
  `unix://` socket under `/tmp`.
- Created a real persisted App Server thread with `thread/start`.
- Confirmed the thread appeared in `thread/loaded/list`.
- Stopped only the disposable server PID at the end of each run.

## Positive admission

Command:

```sh
COLLAB_APPSERVER_SOCKET=<isolated.sock> \
COLLAB_APPSERVER_NAMESPACE=codex_app \
CODEX_THREAD_ID=<loaded-thread> \
cargo test --manifest-path collab/Cargo.toml \
  client::adapters::codex_app_server::tests::live_appserver_candidate_is_admitted_only_when_loaded \
  -- --nocapture
```

Result: `1 passed`, with the candidate admitted only when the real thread was
present in `thread/loaded/list`.

## Negative admission

The same command was run against a syntactically valid thread id absent from
`thread/loaded/list`.

Result: `1 passed`; the test observed `RouteUnavailable` with
`persisted but not loaded`.

## Explicit notification

Command:

```sh
COLLAB_APPSERVER_SOCKET=<isolated.sock> \
COLLAB_APPSERVER_NAMESPACE=codex_app \
CODEX_THREAD_ID=<loaded-thread> \
cargo test --manifest-path collab/Cargo.toml \
  client::adapters::codex_app_server::tests::live_immediate_notify_accepts_loaded_thread \
  -- --ignored --nocapture
```

Result: `1 passed`; `turn/start` accepted the bounded explicit notification
against the real isolated App Server thread.

## Not-loaded notification rejection

`immediate_notify` rejects a persisted but not-loaded thread with
`RouteUnavailable` before issuing `thread/resume` or `turn/start`. The focused
adapter test `immediate_notify_rejects_not_loaded_thread_without_resume_or_turn_start`
asserts that no second App Server method is sent for `notLoaded` metadata,
`thread not loaded`, or `thread not found` responses.

## Focused source checks

```sh
cargo fmt --manifest-path collab/Cargo.toml --all -- --check
cargo test --manifest-path collab/Cargo.toml \
  adapters::codex_app_server::tests -- --test-threads=1
git diff --check
```

Result: formatting and diff checks passed; the adapter test set reported
`29 passed, 0 failed, 1 ignored`.

The full `--all-targets` run also executed all tests; the only failure was the
pre-existing timing-sensitive `subagent::tests::probes_are_bounded_and_require_exact_success`
fixture, which passed when rerun alone. That fixture is outside this candidate
change and is not used as evidence for the loaded-admission contract.
