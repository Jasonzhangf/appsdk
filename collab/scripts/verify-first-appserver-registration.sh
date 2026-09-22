#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
collab_bin=${COLLAB_BIN:-"$repo_root/target/debug/collab"}

if [ ! -x "$collab_bin" ]; then
  printf 'collab binary is not executable: %s\n' "$collab_bin" >&2
  exit 1
fi
if ! command -v codex >/dev/null 2>&1; then
  printf 'codex CLI is required\n' >&2
  exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
  printf 'python3 is required\n' >&2
  exit 1
fi

root=$(mktemp -d /tmp/cg.XXXXXX)
home="$root/h"
codex_home="$root/c"
project="$root/p"
state="$root/s"
socket="$root/a.sock"
thread_file="$root/thread-id"
session_file="$root/session-id"
holder_error_file="$root/thread-holder.error"
appserver_pid=
daemon_pid=
thread_holder_pid=

# This gate must prove first registration from an isolated host identity. Drop
# any inherited production endpoint/session selection before an isolated
# process starts; the real host session identity is read back from thread/start
# and exported below.
unset CODEX_SESSION_ID CODEX_THREAD_ID COLLAB_WORKER
unset COLLAB_APPSERVER_SOCKET COLLAB_APPSERVER_NAMESPACE
unset COLLAB_STATE_DIR
# Collab host-endpoint overrides take precedence over the state-root socket and
# lock in collab/src/scope.rs, and the adapters accept their own endpoint
# overrides; inheriting any of these would steer the gate outside its root.
unset COLLAB_SOCKET_PATH COLLAB_HOST_SOCKET COLLAB_LOCK_PATH COLLAB_HOST_LOCK
unset CODEX_APP_SERVER_SOCKET COLLAB_APPSERVER

cleanup() {
  if [ -n "$thread_holder_pid" ] && kill -0 "$thread_holder_pid" 2>/dev/null; then
    kill -TERM "$thread_holder_pid" 2>/dev/null || true
    wait "$thread_holder_pid" 2>/dev/null || true
  fi
  if [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null; then
    kill -TERM "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  if [ -n "$appserver_pid" ] && kill -0 "$appserver_pid" 2>/dev/null; then
    kill -TERM "$appserver_pid" 2>/dev/null || true
    wait "$appserver_pid" 2>/dev/null || true
  fi
  if [ "${KEEP_GATE_ROOT:-0}" != "1" ] && [ -n "$root" ] && [ -d "$root" ]; then
    case "$root" in
      /tmp/cg.*)
        # The App Server may still be flushing CODEX_HOME when the trap
        # runs; a transient non-empty directory must not fail a PASS gate.
        attempt=0
        while [ -d "$root" ] && [ "$attempt" -lt 20 ]; do
          /bin/rm -r -- "$root" 2>/dev/null && break
          attempt=$((attempt + 1))
          sleep 0.1
        done
        ;;
    esac
  fi
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$home" "$codex_home" "$project" "$state"

HOME="$home" CODEX_HOME="$codex_home" \
  codex app-server --listen "unix://$socket" -c thread_unload_delay_secs=30 \
  >"$root/app-server.out" 2>"$root/app-server.err" &
appserver_pid=$!

attempt=0
while [ ! -S "$socket" ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -gt 100 ]; then
    printf 'isolated App Server did not create %s\n' "$socket" >&2
    exit 1
  fi
  sleep 0.1
done

python3 - "$socket" "$project" "$thread_file" "$session_file" "$holder_error_file" \
  >"$root/thread-holder.out" 2>"$root/thread-holder.err" <<'PY' &
import base64
import json
import os
import socket
import struct
import sys
import time

socket_path, project_root, thread_file, thread_session_file, error_file = sys.argv[1:]
try:
    stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    stream.settimeout(10)
    stream.connect(socket_path)
    key = base64.b64encode(os.urandom(16)).decode()
    request = (
        "GET / HTTP/1.1\r\n"
        "Host: localhost\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n\r\n"
    )
    stream.sendall(request.encode())
    response = b""
    while b"\r\n\r\n" not in response:
        response += stream.recv(4096)
    if not response.startswith(b"HTTP/1.1 101"):
        raise SystemExit(f"websocket upgrade failed: {response!r}")

    def frame(value):
        payload = json.dumps(value, separators=(",", ":")).encode()
        mask = os.urandom(4)
        length = len(payload)
        output = bytearray([0x81])
        if length < 126:
            output.append(0x80 | length)
        elif length <= 0xFFFF:
            output.extend([0x80 | 126])
            output.extend(struct.pack("!H", length))
        else:
            output.extend([0x80 | 127])
            output.extend(struct.pack("!Q", length))
        output.extend(mask)
        output.extend(bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload)))
        return bytes(output)

    buffer = b""

    def read_exact(length):
        global buffer
        while len(buffer) < length:
            buffer += stream.recv(65536)
        value = buffer[:length]
        buffer = buffer[length:]
        return value

    def receive():
        header = read_exact(2)
        opcode = header[0] & 0x0F
        length = header[1] & 0x7F
        if length == 126:
            length = struct.unpack("!H", read_exact(2))[0]
        elif length == 127:
            length = struct.unpack("!Q", read_exact(8))[0]
        mask = read_exact(4) if header[1] & 0x80 else None
        payload = read_exact(length)
        if mask:
            payload = bytes(
                byte ^ mask[index % 4] for index, byte in enumerate(payload)
            )
        if opcode == 0x9:
            return receive()
        return payload

    def call(request_id, method, params):
        stream.sendall(
            frame(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "method": method,
                    "params": params,
                }
            )
        )
        while True:
            message = json.loads(receive())
            if message.get("id") == request_id:
                if "error" in message:
                    raise SystemExit(f"{method} failed: {message['error']}")
                return message["result"]

    call(
        1,
        "initialize",
        {
            "clientInfo": {"name": "collab-gate", "title": "Collab gate", "version": "1.0"},
            "capabilities": {"experimentalApi": True},
        },
    )
    stream.sendall(frame({"method": "initialized", "params": {}}))
    started = call(2, "thread/start", {"cwd": project_root})["thread"]
    thread_id = started.get("id")
    session_id = started.get("sessionId")
    if not isinstance(thread_id, str) or not thread_id.strip():
        raise SystemExit(f"thread/start response is missing thread.id: {started!r}")
    if not isinstance(session_id, str) or not session_id.strip():
        raise SystemExit(
            f"thread/start response is missing thread.sessionId: {started!r}"
        )
    loaded = call(3, "thread/loaded/list", {})["data"]
    if thread_id not in loaded:
        raise SystemExit(f"thread {thread_id} is not loaded: {loaded!r}")
    with open(thread_file, "w", encoding="utf-8") as output:
        output.write(thread_id)
    with open(thread_session_file, "w", encoding="utf-8") as output:
        output.write(session_id)
    time.sleep(120)
except BaseException as error:
    with open(error_file, "w", encoding="utf-8") as output:
        output.write(f"{type(error).__name__}: {error}")
    raise
PY
thread_holder_pid=$!

report_holder_failure() {
  if [ -s "$holder_error_file" ]; then
    printf 'isolated App Server identity failed: %s\n' \
      "$(cat "$holder_error_file")" >&2
    exit 1
  fi
}

attempt=0
while [ ! -s "$thread_file" ]; do
  report_holder_failure
  attempt=$((attempt + 1))
  if [ "$attempt" -gt 100 ]; then
    printf 'isolated App Server did not expose a loaded thread\n' >&2
    exit 1
  fi
  sleep 0.1
done
attempt=0
while [ ! -s "$session_file" ]; do
  report_holder_failure
  attempt=$((attempt + 1))
  if [ "$attempt" -gt 100 ]; then
    printf 'isolated App Server did not expose a host session identity\n' >&2
    exit 1
  fi
  sleep 0.1
done
thread_id=$(cat "$thread_file")
session_id=$(cat "$session_file")
if [ -z "$thread_id" ] || [ -z "$session_id" ]; then
  printf 'isolated identity is incomplete: thread=%s session=%s\n' \
    "$thread_id" "$session_id" >&2
  exit 1
fi

(
  cd "$project"
  HOME="$home" COLLAB_STATE_DIR="$state" "$collab_bin" serve
) \
  >"$root/collab.out" 2>"$root/collab.err" &
daemon_pid=$!

attempt=0
while [ ! -S "$state/server.sock" ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -gt 100 ]; then
    printf 'isolated Collab daemon did not create %s\n' "$state/server.sock" >&2
    exit 1
  fi
  sleep 0.1
done

export HOME="$home"
export COLLAB_STATE_DIR="$state"
export COLLAB_APPSERVER_SOCKET="$socket"
export COLLAB_APPSERVER_NAMESPACE=codex_app
export CODEX_THREAD_ID="$thread_id"
export CODEX_SESSION_ID="$session_id"

cd "$project"
"$collab_bin" init >"$root/init.json"
"$collab_bin" context >"$root/context.json"
"$collab_bin" task status >"$root/task-status.json"

python3 - "$root/init.json" "$root/context.json" "$root/task-status.json" <<'PY'
import json
import sys

init_path, context_path, task_status_path = sys.argv[1:]
with open(init_path, encoding="utf-8") as source:
    init = json.load(source)
with open(context_path, encoding="utf-8") as source:
    context = json.load(source)
with open(task_status_path, encoding="utf-8") as source:
    task_status = json.load(source)

if init.get("ok") is not True:
    raise SystemExit(f"init assertion failed: {init!r}")
liveness = context.get("liveness")
if (
    context.get("registered") is not True
    or not isinstance(liveness, dict)
    or liveness.get("live") is not True
    or liveness.get("presence") != "present"
):
    raise SystemExit(f"context assertion failed: {context!r}")
if task_status.get("tasks") != []:
    raise SystemExit(f"task status assertion failed: {task_status!r}")
PY

printf 'isolated_appserver_first_registration=PASS\n'
printf 'thread_id=%s\n' "$thread_id"
printf 'session_id=%s\n' "$session_id"
if [ "${KEEP_GATE_ROOT:-0}" = "1" ]; then
  printf 'temp_root=%s\n' "$root"
else
  printf 'temp_root=%s (removed on exit)\n' "$root"
fi
