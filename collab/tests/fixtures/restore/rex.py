#!/usr/bin/env python3
"""Record the public CLI calls and emulate an isolated Rex session."""
import json
import os
import sys
import time
from pathlib import Path

root = Path(os.environ["RESTORE_FIXTURE_ROOT"])
mode = os.environ["RESTORE_FIXTURE_MODE"]
args = sys.argv[1:]
assert args[:3] == ["--server", "unix:///fixture/rex.sock", "--autostart=false"]
assert args[3] == "--timeout"
args = args[5:]
assert args[:2] == ["api", "call"] or args[0] == "kill"
method = args[-2] if args[0] == "api" else args[0]
body = json.load(sys.stdin) if args[0] == "api" else {}
sid = "session:30000000-0000-0000-0000-000000000001"
bid = "block:30000000-0000-0000-0000-000000000002"
wid = "window:30000000-0000-0000-0000-000000000003"
lid = "layer:30000000-0000-0000-0000-000000000004"
active = root / "active-session"
label = "restore-fixture"
response = {}
status = 0

preflight_method = {
    "cancel_preflight_status": "server.status",
    "cancel_preflight_list": "session.list",
}.get(mode)
if mode == "cancel_preflight" and method == "server.status":
    preflight_method = method
if method == preflight_method:
    (root / "preflight-child-pid").write_text(str(os.getpid()))
    (root / "preflight-inflight").write_text(method)
    release = root / "release-create"
    deadline = time.monotonic() + 10
    while not release.exists():
        if time.monotonic() >= deadline:
            raise RuntimeError("fixture preflight release marker timed out")
        time.sleep(0.01)

if method == "server.status":
    response = {"host_id": "fixture-host", "instance_id": "fixture-instance"}
    if mode == "status_failure":
        response = {"code": "fixture_status_failure", "message": "fixture native status failure"}
        status = 1
elif method == "session.list":
    response = {"sessions": []}
    if mode == "list_failure":
        response = {"code": "fixture_list_failure", "message": "fixture native list failure"}
        status = 1
    if active.exists():
        response["sessions"].append({"label": label, "session_id": sid})
    elif mode == "existing":
        response["sessions"].append({"label": label, "session_id": sid})
elif method == "session.create":
    (root / "create-child-pid").write_text(str(os.getpid()))
    assert body == {
        "label": label,
        "initial_windows": [{
            "window_label": "CodexAgent",
            "layout": {"block": {
                "label": "shell", "flavor": "com.superlogical.terminal.shell"
            }}
        }],
    }
    if mode == "atomic_failure":
        response = {"code": "bad_request", "message": "fixture initial window failed"}
        status = 1
    elif mode == "create_unknown_failure":
        active.write_text(sid)
        response = {"message": "server created a session but failed before the call could be classified"}
        status = 1
    else:
        if mode in {"cancel_after_create", "cancel_after_create_kill_failure", "cancel_unusable_reply", "held_cleanup"}:
            (root / "create-inflight").write_text(sid)
            release = root / "release-create"
            deadline = time.monotonic() + 10
            while not release.exists():
                if time.monotonic() >= deadline:
                    raise RuntimeError("fixture create release marker timed out")
                time.sleep(0.01)
        active.write_text(sid)
        response = {"session_id": sid, "initial_windows": [
            {"window_id": wid, "block_ids": [bid], "revision": 1},
        ]}
        if mode == "bad_receipt":
            response["initial_windows"] = []
        if mode == "cancel_unusable_reply":
            response = {"message": "create receipt lost its owned session ID"}
elif method == "session.view":
    assert active.read_text() == sid
    assert args == ["api", "call", "-s", sid, "session.view", "-"]
    assert body == {"session_id": sid}
    block = {
        "block_id": bid, "label": "shell", "host_id": "fixture-host",
        "creator_name": "com.superlogical.terminal",
        "flavor": "com.superlogical.terminal.shell",
    }
    layer = {
        "layer_id": lid, "kind": "tiled", "bounds": [0, 0, 1, 1],
        "layout": {"rect": [0, 0, 1, 1], "block_id": bid}, "blocks": [block],
    }
    window = {
        "window_id": wid, "label": "CodexAgent", "focused_block_id": bid,
        "layers": [layer],
    }
    response = {
        "session_id": sid, "revision": 1, "label": label,
        "active_window_id": wid, "windows": [window],
    }
    if mode in {"bad_view", "bad_view_kill_failure"}:
        layer["layout"]["rect"] = [0, 0, 0.5, 1]
    if mode == "changed_binding":
        path = root / "identity-map.json"
        control = json.loads(path.read_text())
        control["slots"][0]["current"]["host"]["profile"] = "changed"
        path.write_text(json.dumps(control))
elif method == "kill":
    assert args == ["kill", sid]
    assert active.read_text() == sid
    if mode in {"kill_failure", "bad_view_kill_failure", "cancel_after_create_kill_failure"}:
        print("fixture kill timed out", file=sys.stderr)
        status = 124
    else:
        active.unlink()
else:
    raise AssertionError(args)

with (root / "receipts.jsonl").open("a") as log:
    print(json.dumps({
        "method": method, "args": args, "body": body, "response": response,
        "exit": status,
    }), file=log)
if mode == "create_response_lost" and method == "session.create":
    print('{"session_id": "session:response-truncated"')
else:
    print(json.dumps(response))
sys.exit(status)
