# Explicit send App Server rejection verification evidence

Base: `421239302f63685250e83ebc0e0fdb4746c4d87f`
Candidate: `e3151d02324759c753fc2550bcbd766e9b1052d4`
Environment: macOS arm64, Codex CLI `0.154.0`, Collab `0.2.0029`

The review scope binds this evidence to the exact candidate commit and tree.

## Isolation

- Started a disposable `codex app-server` with a temporary `CODEX_HOME` and
  `unix://` socket under `/tmp`.
- Used a temporary `COLLAB_STATE_DIR`; no existing `~/.collab` route, daemon,
  socket, or terminal session was used.
- Created real persisted App Server threads with `thread/start`.
- Stopped only the disposable App Server PID at the end of the run.

## Accepted explicit send

The isolated peer was registered through `collab init` with the real loaded
thread. `collab sendmessage` then returned:

```json
{
  "durable": true,
  "msg_id": "m1789916310261-1",
  "notification": "sent",
  "task_id": null
}
```

The App Server `turn/start` receipt was accepted, and `collab recv` returned
the same durable message body and subject.

## Rejected explicit send

After the registered target became unavailable, a repeated explicit send
returned a non-zero result with the durable message ID retained:

```text
collab: APPSERVER_NOTIFICATION_REJECTED: notification was not attempted for the selected App Server transport
```

The response projection was:

```json
{
  "durable": true,
  "notification": "subscribed-not-sent",
  "notification_error": "notification was not attempted for the selected App Server transport"
}
```

This is an explicit failure, not a successful `subscribed-not-sent` response.
When the transport was positively unavailable before admission, the command
failed earlier with `recipient has no live server-verified transport`; it did
not claim notification success.

## Focused source checks

```sh
cargo fmt --manifest-path collab/Cargo.toml --all -- --check
cargo test --manifest-path collab/Cargo.toml \
  server::peer_tests::explicit_send_ \
  -- --test-threads=1
git diff --check
```

Result: formatting and diff checks passed; the focused regression tests
reported `3 passed, 0 failed`.
