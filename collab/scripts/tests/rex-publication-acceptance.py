"""Observe retry after the isolated daemon's public Close publication barrier."""

import http.server
import json
import pathlib
import shlex
import signal
import subprocess
import sys
import tempfile
import threading
import time


def wait_until(observe, description, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = observe()
        if result:
            return result
        time.sleep(0.1)
    raise RuntimeError(description)


class LocalModel(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        metadata = self.headers.get("x-codex-turn-metadata") or request.get(
            "client_metadata", {}
        ).get("x-codex-turn-metadata")
        self.server.requests.append((json.loads(metadata) if metadata else {}, request))
        response_id = f"fixture-{len(self.server.requests)}"
        events = [
            {"type": "response.output_item.done", "item": {
                "type": "message", "id": response_id + "-message", "role": "assistant",
                "content": [{"type": "output_text", "text": "Isolated fixture turn complete"}],
            }},
            {"type": "response.completed", "response": {"id": response_id}},
        ]
        data = "".join(f"data: {json.dumps(event)}\n\n" for event in events).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_):
        pass


def verify(collab, rex, codex, project, state, gate, root, model):
    home = root / "home"
    codex_home = root / "codex"
    home.mkdir()
    codex_home.mkdir()
    env = {
        "HOME": str(home), "CODEX_HOME": str(codex_home), "COLLAB_STATE_DIR": str(state),
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "TERM": "xterm-256color", "NO_PROXY": "*",
    }
    (codex_home / "config.toml").write_text(f"""
model = "fixture"
model_provider = "fixture"
check_for_update_on_startup = false
[model_providers.fixture]
name = "Fixture"
base_url = "http://127.0.0.1:{model.server_port}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
[analytics]
enabled = false
[features]
plugins = false
remote_plugin = false
[projects."{project}"]
trust_level = "trusted"
""")
    socket = root / "rex.sock"
    endpoint = f"unix://{socket}"

    def run(args, payload=None):
        result = subprocess.run(
            args, env=env, cwd=project, text=True, capture_output=True, timeout=20,
            input=json.dumps(payload) if payload is not None else None,
        )
        assert result.returncode == 0, (args[0], result.returncode, result.stderr[-1000:])
        return result.stdout

    def rex_call(*args, payload=None):
        return run([rex, "--server", endpoint, "--autostart=false", *args], payload)

    server = subprocess.Popen(
        [rex, "server", "run", "--listen", endpoint, "--state-dir", str(root / "rex-state"),
         "--restore=false"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    session = None
    try:
        wait_until(socket.exists, "isolated Rex did not listen")
        command = shlex.join([
            "/usr/bin/env", "-i", *(f"{key}={value}" for key, value in env.items()),
            f"REX_SERVER={endpoint}",
        ])
        invocation = shlex.join([
            codex, "--no-daemon", "--no-alt-screen", "--ask-for-approval", "never",
            "--sandbox", "workspace-write",
        ])
        created = json.loads(rex_call("api", "call", "session.create", "-", payload={
            "label": project.name,
            "initial_windows": [{"window_label": "test", "layout": {"block": {
                "flavor": "com.superlogical.terminal.shell", "options": {
                    "shell": "none", "login": False, "cwd": str(project),
                    "command": ["/bin/sh", "-c", f'exec {command} REX_SESSION="$REX_SESSION" '
                                f'REX_BLOCK="$REX_BLOCK" {invocation}'],
                },
            }}}],
        }))
        session = created["session_id"]
        block = created["initial_windows"][0]["block_ids"][0]
        target = ("-s", session, "-b", block)

        def capture():
            return rex_call("capture", *target, "--unwrap")

        def write(text):
            rex_call("api", "call", "-s", session, "com.superlogical.terminal.write", "-",
                     payload={"session_id": session, "block_id": block, "args": {"data": text}})

        def enter():
            rex_call("send-key", *target, "enter")

        def terminal_json(args, stem):
            receipt = project / (stem + ".json")
            output = receipt.with_suffix(".tmp")
            error = project / (stem + ".err")
            script = project / (stem + ".sh")
            # Codex shell mode may omit the parent Rex environment. Use the
            # exact isolated placement returned by the session creation.
            command = ["/usr/bin/env", f"REX_SERVER={endpoint}",
                       f"REX_SESSION={session}", f"REX_BLOCK={block}", *args]
            script.write_text(f"{shlex.join(command)} > {shlex.quote(str(output))} "
                              f"2> {shlex.quote(str(error))} && "
                              f"mv {shlex.quote(str(output))} {shlex.quote(str(receipt))}\n")
            write("!")
            wait_until(lambda: "shell mode" in capture().lower(), "Codex did not enter shell mode")
            write(shlex.join(["/bin/sh", str(script)]))
            enter()
            try:
                return wait_until(lambda: json.loads(receipt.read_text())
                                  if receipt.exists() else None, stem)
            except RuntimeError:
                detail = error.read_text()[-1000:] if error.exists() else ""
                if not detail and output.exists():
                    detail = output.read_text()[-1000:]
                raise RuntimeError(f"{stem} did not return JSON: {detail}") from None

        wait_until(lambda: "fixture" in capture(), "Codex did not reach its composer")
        write("Establish the isolated Rex session")
        enter()
        wait_until(lambda: "Isolated fixture turn complete" in capture(), "initial turn did not finish")
        context = terminal_json([collab, "context"], "publication-context")["result"]
        assert context["outcome"] == "completed", context.get("requires")
        snapshot = context["snapshot"]
        assert snapshot["identity"]["transport"]["rex_endpoint"]["block_id"] == block
        agent_session = snapshot["session_id"]
        gate.with_suffix(".begin").touch()
        wait_until(gate.with_suffix(".held").exists, "Close did not reach its held archive")

        body = "original body after binding publication"
        deferred = json.loads(run([
            collab, "send", "--unknown-sender", "--to", f"{project.name}.{snapshot['name']}",
            "--delivery", "immediate", "--subject", "Rex real publication retry", body,
        ]))
        message_id = deferred["msg_id"]
        print(json.dumps(deferred), flush=True)
        assert deferred["notification"] == "queued", deferred
        assert not ({"notification_error", "failure", "repair_required", "escalation"} &
                    deferred.keys()), deferred
        assert deferred["durable"] and not deferred["consumed"], deferred

        def native_turns():
            return {metadata["turn_id"] for metadata, request in model.requests
                    if metadata.get("thread_id") == agent_session and metadata.get("turn_id")
                    and any(item.get("role") == "user" and message_id in json.dumps(item)
                            for item in request.get("input", []))}

        assert not native_turns()
        gate.with_suffix(".release").touch()
        wait_until(native_turns, "publication retry did not start a native turn", 30)
        wait_until(lambda: "Isolated fixture turn complete" in capture(), "native turn did not finish")
        recv = [collab, "recv"]
        receipt = terminal_json(recv, "publication-receive")
        assert any(msg["id"] == message_id and msg["body"] == body for msg in receipt["messages"])
        replay = terminal_json(recv, "publication-receive-replay")
        assert not any(msg["id"] == message_id for msg in replay["messages"])
        assert len(native_turns()) == 1
        worker_id = snapshot["identity"]["worker_id"]
        status = terminal_json([collab, "worker", "status", worker_id], "publication-worker-status")
        worker = next(row for row in status.get("workers", [status]) if row["id"] == worker_id)
        facts = {key: worker.get(key) for key in ("identity_valid", "presence", "endpoint_live", "agent_state")}
        assert facts["identity_valid"] and facts["presence"] == "present" and facts["endpoint_live"], facts
        print(json.dumps({"status_after_native_receive": facts}), flush=True)
        print(json.dumps({"message_id": message_id, "exact_body": True, "native_turns": 1,
                          "consumed": True, "recv_replay": 0}), flush=True)

        # A genuine native failure must still be visible. Pause only this
        # fixture's Rex server so status cannot complete before the write.
        server.send_signal(signal.SIGSTOP)
        try:
            rejected = subprocess.run([
                collab, "send", "--unknown-sender", "--to", f"{project.name}.{snapshot['name']}",
                "--delivery", "immediate", "--subject", "Rex status rejection",
                "Rex status must respond before native input",
            ], env=env, cwd=project, text=True, capture_output=True, timeout=20)
        finally:
            server.send_signal(signal.SIGCONT)
        assert rejected.returncode == 1, (rejected.returncode, rejected.stdout, rejected.stderr)
        rejected_receipt = next(
            json.loads(line.removeprefix("collab response: "))
            for line in rejected.stderr.splitlines() if line.startswith("collab response: ")
        )
        assert rejected_receipt["durable"] and rejected_receipt["msg_id"], rejected_receipt
        assert rejected_receipt["notification"] == "subscribed-not-sent", rejected_receipt
        assert rejected_receipt["notification_error"].startswith(
            "REX_NOT_DELIVERED: operation=server.status stage=BeforeWrite"
        ), rejected_receipt
        print(json.dumps({"send_exit": rejected.returncode, "rejection": rejected_receipt}), flush=True)
    finally:
        gate.with_suffix(".release").touch()
        try:
            if session and server.poll() is None:
                rex_call("api", "call", "-s", session, "session.destroy", "-",
                         payload={"session_id": session})
                inventory = json.loads(rex_call("ls", "--json"))
                assert all(entry["session_id"] != session for entry in inventory["sessions"]), (
                    f"isolated Rex session still present: {session}"
                )
                print(json.dumps({"cleanup": "rex-session-absent", "session_id": session}), flush=True)
        finally:
            if server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
            print(json.dumps({"cleanup": "rex-server-exited", "pid": server.pid}), flush=True)


def cancel_test(signum, _frame):
    raise SystemExit(128 + signum)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, cancel_test)
    cli, rex_cli, codex_cli, project_dir, state_dir, gate_path = sys.argv[1:]
    with tempfile.TemporaryDirectory(prefix="collab-rex-publication-", dir="/private/tmp") as work:
        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), LocalModel) as model:
            model.requests = []
            thread = threading.Thread(target=model.serve_forever)
            thread.start()
            try:
                verify(cli, rex_cli, codex_cli, pathlib.Path(project_dir), pathlib.Path(state_dir),
                       pathlib.Path(gate_path), pathlib.Path(work), model)
            finally:
                model.shutdown()
                thread.join()
