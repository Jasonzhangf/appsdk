"""Opt-in Collab Context test with a real iTerm2 client and a private tmux server."""

import asyncio
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

import iterm2


async def wait_for(read):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        result = read()
        if result:
            return result
        await asyncio.sleep(0.05)
    raise RuntimeError("Timed out waiting for isolated Collab or tmux")


async def check(connection):
    executable = Path(sys.argv[1]).resolve(strict=True)
    tmux_binary = shutil.which("tmux")
    assert tmux_binary
    with tempfile.TemporaryDirectory(prefix="collab-iterm-", dir="/private/tmp") as tmp:
        root = Path(tmp)
        project = root / "project"
        state = root / "state"
        project.mkdir()
        state.mkdir()
        (project / ".agent-collab").mkdir()
        tmux_socket = root / "tmux.sock"
        (state / "tmux-endpoints.json").write_text(json.dumps({"endpoints": [str(tmux_socket)]}))
        env = {key: os.environ[key] for key in ("HOME", "USER", "LOGNAME", "PATH", "TERM") if key in os.environ}
        env["COLLAB_STATE_DIR"] = str(state)
        env["CODEX_HOME"] = str(root / "codex")
        env["CODEX_INTERNAL_ORIGINATOR_OVERRIDE"] = "Codex TUI"

        def tmux(*args):
            return subprocess.run(
                [tmux_binary, "-S", str(tmux_socket), *args], env=env, text=True,
                check=True, capture_output=True,
            ).stdout.strip()

        async def context(phase):
            (root / f"go-{phase}").touch()
            try:
                await wait_for((root / f"done-{phase}").exists)
            except RuntimeError as error:
                raise RuntimeError(f"Timed out during Collab fixture phase {phase}") from error
            status = int((root / f"done-{phase}").read_text())
            output = (root / f"out-{phase}").read_text()
            return status, json.loads(output) if output else {}, (root / f"err-{phase}").read_text()

        def start_session(number, agent_session, phases, context_args=()):
            runner = root / f"context-{number}.sh"
            runner.write_text(
                "set -eu\n"
                f"cd {shlex.quote(str(project))}\n"
                f"export CODEX_SESSION_ID={agent_session}\n"
                f"root={shlex.quote(str(root))}\n"
                f"for phase in {shlex.join(phases)}; do\n"
                "  while [ ! -f \"$root/go-$phase\" ]; do sleep .03; done\n"
                "  status=0\n"
                "  if [ \"$phase\" = old-query ]; then\n"
                f"    {shlex.quote(str(executable))} context --query >\"$root/out-$phase\" 2>\"$root/err-$phase\" || status=$?\n"
                "  elif [ \"$phase\" = partially-attached ]; then\n"
                f"    {shlex.quote(str(executable))} worker status >\"$root/out-$phase\" 2>\"$root/err-$phase\" || status=$?\n"
                "  elif [ \"$phase\" = schedule ]; then\n"
                f"    {shlex.quote(str(executable))} notify subscribe --event deadline --subject fixture-deadline --at-ms \"$(cat \"$root/due-ms\")\" --ttl-seconds 180 >\"$root/out-$phase\" 2>\"$root/err-$phase\" || status=$?\n"
                "  elif [ \"$phase\" = force ]; then\n"
                f"    {shlex.quote(str(executable))} context --approve-identity \"$(cat \"$root/force\")\" >\"$root/out-$phase\" 2>\"$root/err-$phase\" || status=$?\n"
                "  elif [ \"$phase\" = delivery ]; then\n"
                "    : >\"$root/err-$phase\"\n"
                "    message=$(cat \"$root/message-id\")\n"
                "    for round in 1 2 3 4 5 6 7 8 9 10; do\n"
                f"      {shlex.quote(str(executable))} msg \"$message\" >\"$root/out-$phase\"\n"
                "      if grep -q '\"text_submitted\": true' \"$root/out-$phase\"; then break; fi\n"
                "      sleep .4\n"
                "    done\n"
                f"    {shlex.quote(str(executable))} recv >\"$root/received.json\"\n"
                f"    {shlex.quote(str(executable))} notify status >\"$root/subscriptions.json\"\n"
                "  else\n"
                f"    {shlex.join([str(executable), 'context', *context_args])} >\"$root/out-$phase\" 2>\"$root/err-$phase\" || status=$?\n"
                "  fi\n"
                f"  if [ \"$phase\" = again ]; then {shlex.quote(str(executable))} worker status >\"$root/status.json\"; fi\n"
                "  printf '%d\\n' \"$status\" >\"$root/tmp-$phase\"\n"
                "  mv \"$root/tmp-$phase\" \"$root/done-$phase\"\n"
                "done\n"
                "exec /bin/sleep 120\n"
            )
            return tmux("-f", "/dev/null", "new-session", "-dP", "-F", "#{session_id}", "-s", number,
                        shlex.join(["/bin/sh", str(runner)]))

        async def attach(session):
            command = shlex.join([tmux_binary, "-S", str(tmux_socket), "attach-session", "-t", session])
            window = await iterm2.Window.async_create(connection, command=command)
            current = window.tabs[0].sessions[0]
            for _ in range(100):
                tty = await current.async_get_variable("session.tty")
                rows = tmux("list-clients", "-F", "#{client_tty} #{session_id}").splitlines()
                if f"{tty} {session}" in rows:
                    return current
                await asyncio.sleep(0.05)
            await current.async_close(force=True)
            raise RuntimeError("iTerm2 and tmux have no matching TTY")

        agent_session = "isolated-iterm-codex-session"
        session = start_session("0", agent_session,
                                ("detached", "attached", "partially-attached", "again", "schedule", "old-query", "force"))
        tmux("set-option", "-s", "exit-empty", "on")
        pane = None
        viewer = None
        successor_pane = None
        successor = None
        with (root / "daemon.log").open("w") as log:
            daemon = subprocess.Popen([executable, "serve"], cwd=project, env=env, stdout=log, stderr=log)
            try:
                await wait_for((state / "server.sock").exists)
                map_path = state / "identity-map.json"
                before = map_path.read_bytes() if map_path.exists() else None
                status, _, error = await context("detached")
                assert status != 0 and "TMUX_ITERM2_PANE_REQUIRED" in error, (status, error)
                assert (map_path.read_bytes() if map_path.exists() else None) == before
                assert tmux("display-message", "-p", "-t", session, "#{session_name}") == "0"

                pane = await attach(session)
                viewer = await attach(session)
                status, value, error = await context("attached")
                assert status == 0, error
                snapshot = value["result"]["snapshot"]
                assert snapshot["registered"] and snapshot["session_id"] == agent_session
                worker_id = snapshot["identity"]["worker_id"]
                name = snapshot["name"]
                assert tmux("display-message", "-p", "-t", session, "#{session_name}") == f"{session[1:]}_{name}"

                request = json.dumps({"action": "restore_name", "requested_name": name,
                                      "authorization": "Isolated fixture selects its existing role"})
                successor = start_session("1", "isolated-iterm-successor", ("collision", "replace", "delivery"),
                                          ("--approve-identity", request))
                successor_pane = await attach(successor)
                bound = map_path.read_bytes()
                status, _, error = await context("collision")
                assert status != 0 and "TMUX_NAME_ONLINE_CONFLICT" in error, (status, error)
                assert map_path.read_bytes() == bound
                assert tmux("display-message", "-p", "-t", successor, "#{session_name}") == "1"

                await pane.async_close(force=True)
                pane = None
                status, workers, error = await context("partially-attached")
                assert status == 0, error
                still_online = next(worker for worker in workers["workers"] if worker["id"] == worker_id)
                assert still_online["endpoint_live"] is True
                await viewer.async_close(force=True)
                viewer = None
                await wait_for(lambda: session not in tmux("list-clients", "-F", "#{session_id}").splitlines())
                status, value, error = await context("again")
                assert status == 0, error
                assert value["result"]["snapshot"]["identity"]["worker_id"] == worker_id
                workers = json.loads((root / "status.json").read_text())["workers"]
                saved = next(worker for worker in workers if worker["id"] == worker_id)
                assert saved["status"] == "offline" and saved["endpoint_live"] is False
                (root / "due-ms").write_text(str(time.time_ns() // 1_000_000 + 60_000))
                status, scheduled, error = await context("schedule")
                assert status == 0, error
                schedule = scheduled["subscription"]

                sent = subprocess.run(
                    [executable, "send", "--unknown-sender", "--to", f"{project.name}.{name}",
                     "--subject", "offline fixture", "isolated offline message"], cwd=project,
                    env=env, text=True, capture_output=True, timeout=10,
                )
                assert sent.stdout, sent.stderr
                receipt = json.loads(sent.stdout)
                assert sent.returncode == 0 and receipt["durable"] and receipt["notification"] == "queued"
                message_id = receipt["msg_id"]
                (root / "message-id").write_text(message_id)

                status, value, error = await context("replace")
                assert status == 0, error
                replaced = value["result"]["snapshot"]
                assert replaced["name"] == name
                assert replaced["identity"]["worker_id"] != worker_id
                assert replaced["identity"]["transport"]["tmux_endpoint"]["tmux_session_id"] == successor
                names = dict(row.split() for row in tmux("list-sessions", "-F", "#{session_id} #{session_name}").splitlines())
                assert names[session].isdigit() and names[successor] == f"{successor[1:]}_{name}"
                status, delivered, error = await context("delivery")
                assert status == 0, error
                assert delivered["wake_transport_evidence"]["text_submitted"] is True
                received = json.loads((root / "received.json").read_text())
                assert message_id in json.dumps(received) and "isolated offline message" in json.dumps(received)
                subscriptions = json.loads((root / "subscriptions.json").read_text())["subscriptions"]
                deadline = next(item for item in subscriptions if item["id"] == schedule["id"])
                assert deadline["status"] == "armed" and deadline["trigger_ms"] == schedule["trigger_ms"]

                pane = await attach(session)
                status, old_identity, error = await context("old-query")
                assert status != 0 and not old_identity.get("ok") and "IDENTITY_MAP_NOT_FOUND" in error
                (root / "force").write_text(json.dumps({
                    "action": "restore_name", "requested_name": name,
                    "authorization": "Isolated fixture explicitly replaces the live holder",
                    "replace_confirmed": True,
                }))
                status, restored, error = await context("force")
                assert status == 0, error
                snapshot = restored["result"]["snapshot"]
                assert snapshot["name"] == name
                assert snapshot["identity"]["transport"]["tmux_endpoint"]["tmux_session_id"] == session
                print("PASS: detach preserves the logical name, unread mail and deadline follow it, and online replacement requires explicit confirmation")
            finally:
                if successor_pane is not None:
                    await successor_pane.async_close(force=True)
                if pane is not None:
                    await pane.async_close(force=True)
                if viewer is not None:
                    await viewer.async_close(force=True)
                daemon.terminate()
                daemon.wait(timeout=5)
                if successor is not None:
                    tmux("kill-session", "-t", successor)
                tmux("kill-session", "-t", session)


if __name__ == "__main__":
    iterm2.run_until_complete(check, retry=False)
