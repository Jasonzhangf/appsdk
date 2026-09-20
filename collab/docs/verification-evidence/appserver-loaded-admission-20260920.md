# App Server loaded-admission verification evidence

Candidate: `bc48653f060a5a85f5e01f6d07696557a4975276`
Tree: `23703bd9811e06423f481ebe3740acfa98a6bf72`
Base: `384abb8a737525d01aaa10362a78a77d60f7010e`
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
  client::adapters::codex_app_server::tests::live_appserver_candidate_is_admitted_when_loaded_or_rejected_when_unloaded \
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
adapter tests `immediate_notify_rejects_not_loaded_thread` and
`immediate_notify_rejects_thread_read_not_loaded_error` assert that no second
App Server method is sent for `notLoaded` metadata or a `thread not loaded`
response. The separate tests
`immediate_notify_rejects_missing_thread_without_turn_start` and
`immediate_notify_rejects_wrapped_missing_thread_without_turn_start` cover the
missing-thread response variants and their wrapped form.

## Focused source checks

```sh
cargo fmt --manifest-path collab/Cargo.toml --all -- --check
cargo test --manifest-path collab/Cargo.toml \
  adapters::codex_app_server::tests -- --test-threads=1
git diff --check
```

Result: formatting and diff checks passed; the adapter test set reported
`31 passed, 0 failed, 1 ignored`.

The full `--all-targets` run also executed all tests; the only failure was the
pre-existing timing-sensitive `subagent::tests::probes_are_bounded_and_require_exact_success`
fixture, which passed when rerun alone. That fixture is outside this candidate
change and is not used as evidence for the loaded-admission contract.
