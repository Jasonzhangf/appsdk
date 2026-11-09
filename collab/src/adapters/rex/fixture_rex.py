#!/usr/bin/env python3
"""Standalone Rex CLI fixture for the Collab Rex primitive unit tests.

This is test-only preparation. It is not product code and is never reached by
the production adapter. The Rust primitive module injects the CLI path for
tests only; this script models the already-verified official CLI contract so a
test can prove argv/stdin/env/write/enter/timeout behavior without touching a
real Rex server.

Control (all non-REX_ names so the primitive's REX_* strip does not remove them).
Names must NOT start with `REX_` because the primitive strips every inherited
`REX_*` variable before spawning this fixture.
  REXPRIMITIVE_LOG_DIR   directory that receives invocations.jsonl (required)
  REXPRIMITIVE_SCENARIO  scenario name (default: normal)

Each invocation appends one JSON line to invocations.jsonl:
  {"serial":n,"argv":[...],"stdin":"...","rex_env":[...],"cwd":"..."}

Scenarios (override behavior keyed by the extracted method):
  normal                 all calls succeed
  non_terminal_block     session.list_blocks returns a non-terminal creator
  session_missing        session.list omits the requested session
  write_short            write returns bytes_written = 1
  write_unknown          write exits 124 with no JSON (timeout-like)
  write_typed_reject     write exits 1 with a structured native error code
  enter_fail             send-key exits 3 with stderr text
  enter_typed_reject     send-key exits 1 with a structured native error code
  events_stream          events emits matching terminal events then stays open
  events_malformed       events emits one valid event then a broken JSON line
"""

from __future__ import annotations

import json
import os
import sys
import time

SERVER = "unix:///tmp/rex-fixture.sock"
SESSION = "session:11111111-1111-1111-1111-111111111111"
BLOCK = "block:22222222-2222-2222-2222-222222222222"
OTHER_SESSION = "session:33333333-3333-3333-3333-333333333333"
TERMINAL = "com.superlogical.terminal"


def log_dir() -> str:
    value = os.environ.get("REXPRIMITIVE_LOG_DIR")
    if not value:
        # Missing control is a test harness bug, so fail loudly on stderr.
        sys.stderr.write("fixture: REXPRIMITIVE_LOG_DIR is required\n")
        raise SystemExit(90)
    return value


def scenario() -> str:
    return os.environ.get("REXPRIMITIVE_SCENARIO", "normal")


def record(serial: int, argv: list[str], stdin: str) -> None:
    path = os.path.join(log_dir(), "invocations.jsonl")
    rex_env = sorted(key for key in os.environ if key.startswith("REX_"))
    entry = {
        "serial": serial,
        "pid": os.getpid(),
        "argv": argv,
        "stdin": stdin,
        "rex_env": rex_env,
        "cwd": os.getcwd(),
    }
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry, ensure_ascii=False) + "\n")


def next_serial() -> int:
    path = os.path.join(log_dir(), ".serial")
    try:
        with open(path, encoding="utf-8") as handle:
            value = int(handle.read().strip() or "0")
    except FileNotFoundError:
        value = 0
    value += 1
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(str(value))
    return value


def emit(value: object) -> None:
    sys.stdout.write(json.dumps(value, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def extract_method(argv: list[str]) -> str:
    """Return the semantic method the primitive asked for."""
    try:
        api = argv.index("api")
    except ValueError:
        api = -1
    if api >= 0 and argv[api : api + 2] == ["api", "call"]:
        # api call [-s S] <method> -
        rest = argv[api + 2 :]
        if rest[:1] == ["-s"]:
            rest = rest[2:]
        return rest[0] if rest else ""
    for candidate in ("send-key", "events", "version", "capture"):
        if candidate in argv:
            return candidate
    if "api" in argv and "list" in argv:
        return "api.list"
    return ""


def read_stdin() -> str:
    if sys.stdin.isatty():
        return ""
    return sys.stdin.read()


def require_prefix(argv: list[str]) -> None:
    if "--server" not in argv or "--autostart=false" not in argv:
        sys.stderr.write("fixture: missing --server or --autostart=false\n")
        raise SystemExit(6)
    if "--timeout" not in argv:
        sys.stderr.write("fixture: missing --timeout\n")
        raise SystemExit(2)


def server_version() -> None:
    emit(
        {
            "rex_version": "0.1.0",
            "platform": "darwin",
            "dirty": False,
            "api_revision": 11,
            "api_min_revision": 11,
            "host_id": "host-fixture",
            "instance_id": "instance-fixture",
        }
    )


def server_status() -> None:
    if scenario() == "missing_instance":
        emit({"host_id": "host-fixture"})
        return
    emit(
        {
            "rex_version": "0.1.0",
            "platform": "darwin",
            "dirty": False,
            "host_id": "host-fixture",
            "instance_id": "instance-fixture",
        }
    )


def session_list() -> None:
    if scenario() == "session_missing":
        emit({"sessions": []})
        return
    emit({"sessions": [{"session_id": SESSION, "label": "fixture"}, {"session_id": OTHER_SESSION, "label": "other"}]})


def list_blocks() -> None:
    creator = "com.other.creator" if scenario() == "non_terminal_block" else TERMINAL
    emit(
        {
            "blocks": [
                {
                    "block_id": BLOCK,
                    "creator_name": creator,
                    "flavor": "com.superlogical.terminal.shell",
                    "placement": "window",
                    "window_id": "window-fixture",
                    "host_id": "host-fixture",
                }
            ]
        }
    )


def write(stdin: str) -> None:
    mode = scenario()
    if mode == "write_unknown":
        raise SystemExit(124)
    if mode == "write_typed_reject":
        emit({"error": {"code": "session_not_found", "message": "no such session"}})
        raise SystemExit(1)
    try:
        request = json.loads(stdin)
        data = request["args"]["data"]
    except Exception:
        sys.stderr.write("fixture: write request is malformed\n")
        raise SystemExit(7)
    expected = len(data.encode("utf-8"))
    if mode == "write_short":
        emit({"bytes_written": 1})
        return
    emit({"bytes_written": expected})


def send_key() -> None:
    mode = scenario()
    if mode == "enter_fail":
        sys.stderr.write("fixture: enter rejected\n")
        raise SystemExit(3)
    if mode == "enter_typed_reject":
        emit({"error": {"code": "conflict", "message": "session not attached"}})
        raise SystemExit(1)
    emit({})


def process() -> None:
    emit(
        {
            "last_exit": None,
            "child": {
                "pid": 4242,
                "user_id": 501,
                "name": "python3",
                "path": "/usr/bin/python3",
                "cwd": "/tmp/rex-fixture-cwd",
            },
            "foreground": {
                "pid": 4242,
                "user_id": 501,
                "name": "python3",
            },
            "home": "/Users/fixture",
        }
    )


def pwd() -> None:
    emit({"pwd": "file:///tmp/rex-fixture-cwd", "owner": {"pid": 4242, "name": "python3"}})


def program_status() -> None:
    mode = scenario()
    if mode == "status_working":
        emit(
            {
                "records": [
                    {
                        "id": "agent",
                        "state": "working",
                        "app": "fixture",
                        "msg": "busy",
                        "updated_at": "2026-11-08T00:00:00Z",
                    }
                ]
            }
        )
    elif mode == "status_blocked":
        emit(
            {
                "records": [
                    {
                        "id": "agent",
                        "state": "blocked",
                        "kind": "question",
                        "app": "fixture",
                        "msg": "need input",
                        "updated_at": "2026-11-08T00:00:00Z",
                    }
                ]
            }
        )
    else:
        emit({"records": []})


def block_event(name: str, payload: dict) -> dict:
    return {
        "type": "session_event",
        "payload": {
            "session_id": SESSION,
            "event": {
                "type": "block_event",
                "payload": {
                    "block_id": BLOCK,
                    "creator_name": TERMINAL,
                    "name": name,
                    "payload": payload,
                },
            },
        },
    }


def events() -> None:
    mode = scenario()
    if mode == "events_descendant":
        pid = os.fork()
        if pid == 0:
            time.sleep(30)
            os._exit(0)
        with open(os.path.join(log_dir(), "descendant.pid"), "w", encoding="utf-8") as handle:
            handle.write(str(pid))
    if mode == "events_burst":
        for index in range(3):
            emit(block_event("program_status_changed", {"id": str(index), "record": {"state": "working"}}))
        while True:
            time.sleep(0.25)
    if mode == "events_malformed":
        emit(block_event("program_status_changed", {"id": "agent", "record": {"state": "working", "updated_at": "2026-11-08T00:00:00Z"}}))
        sys.stdout.write("{this is not json}\n")
        sys.stdout.flush()
        raise SystemExit(0)
    emit(block_event("program_status_changed", {"id": "agent", "record": {"state": "working", "updated_at": "2026-11-08T00:00:00Z"}}))
    # Stay open so a cancel/deadline path can terminate this owned child.
    while True:
        time.sleep(0.25)


def dispatch(method: str, stdin: str) -> None:
    if method == "version":
        sys.stdout.write("rex fixture 0.1.0\n")
        sys.stdout.flush()
        return
    if method == "server.version":
        server_version()
        return
    if method == "server.status":
        server_status()
        return
    if method == "session.list":
        session_list()
        return
    if method == "session.list_blocks":
        list_blocks()
        return
    if method == f"{TERMINAL}.write":
        write(stdin)
        return
    if method == "send-key":
        send_key()
        return
    if method == f"{TERMINAL}.process":
        process()
        return
    if method == f"{TERMINAL}.pwd":
        pwd()
        return
    if method == f"{TERMINAL}.program_status":
        program_status()
        return
    if method == "events":
        events()
        return
    sys.stderr.write(f"fixture: unsupported method {method!r}\n")
    raise SystemExit(4)


def main() -> int:
    argv = sys.argv[1:]
    require_prefix(argv)
    stdin = read_stdin()
    record(next_serial(), argv, stdin)
    method = extract_method(argv)
    if scenario() == "slow_all":
        time.sleep(1)
    if scenario() == "slow_status" and method == "server.status":
        time.sleep(1.5)
    if scenario() == "observe_slow_second" and method == "com.superlogical.terminal.pwd":
        time.sleep(5)
    dispatch(method, stdin)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
