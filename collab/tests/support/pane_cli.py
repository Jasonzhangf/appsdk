#!/usr/bin/env python3
"""Private tmux-pane command channel for Collab public CLI tests."""

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import termios
import tempfile
import threading
import time
from pathlib import Path


MARKER = "COLLAB_TEST_PANE_REQUEST:"
ENV_KEYS = {
    "PATH",
    "HOME",
    "CODEX_HOME",
    "CODEX_SESSION_ID",
    "CODEX_THREAD_ID",
    "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
    "CODEX_APP_SERVER_SOCKET",
    "COLLAB_STATE_DIR",
    "COLLAB_CONTEXT_CANCEL_HOOK_SOCKET",
    "COLLAB_BINARY",
    "COLLAB_BIN",
    "PROJECT_ROOT",
    "PANE_STDOUT",
    "PANE_STDERR",
}


def allowed_env():
    environment = {
        key: value
        for key, value in os.environ.items()
        if key in ENV_KEYS or key.startswith("COLLAB_TEST_")
    }
    environment["LC_ALL"] = "en_US.UTF-8"
    return environment


def write_atomic(path, value):
    path = Path(path)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(value)
    os.replace(temporary, path)


def ready_path(root, pane_id):
    return Path(root) / f"pane-runner-{pane_id.lstrip('%')}.ready"


def control_tmux():
    path = os.environ.get("PANE_CONTROL_PATH", "")
    resolved = shutil.which("tmux", path=path)
    if resolved is None:
        raise RuntimeError("tmux is unavailable on the private pane control path")
    return resolved


def send_request(socket, pane_id, request):
    tmux = control_tmux()
    subprocess.run(
        # Keep the private frame and its delimiter in one tmux command so a
        # real notification cannot be pasted between the path and Enter.
        [tmux, "-S", socket, "send-keys", "-t", pane_id, "-l", f"{MARKER}{request}\n"],
        check=True,
    )


def run_client(args):
    root = Path(args.root)
    ready = ready_path(root, args.pane_id)
    deadline = time.monotonic() + 10
    while not ready.exists() and time.monotonic() < deadline:
        time.sleep(0.02)
    if not ready.exists():
        print(f"pane runner is not ready for {args.pane_id}", file=sys.stderr)
        return 127

    request_dir = Path(tempfile.mkdtemp(prefix="request-", dir=root))
    stdout = request_dir / "stdout"
    stderr = request_dir / "stderr"
    pid_file = request_dir / "pid"
    exit_file = request_dir / "exit"
    request_file = request_dir / "request.json"
    stdin_file = request_dir / "stdin" if args.pipe_stdin else None
    if stdin_file is not None:
        os.mkfifo(stdin_file)
    stdin = args.args
    if stdin and stdin[0] == "--":
        stdin = stdin[1:]
    request = {
        "kind": "exec",
        "argv": [args.binary, *stdin],
        "cwd": os.getcwd(),
        "env": allowed_env(),
        "requester_pid": os.getpid(),
        "stdout": str(stdout),
        "stderr": str(stderr),
        "pid_file": str(pid_file),
        "exit_file": str(exit_file),
        "stdin": str(stdin_file) if stdin_file is not None else None,
    }
    request_file.write_text(json.dumps(request))
    stdout.touch()
    stderr.touch()
    stop = threading.Event()

    def pump(source, destination):
        with open(source, "rb", buffering=0) as reader:
            while True:
                chunk = reader.read(4096)
                if chunk:
                    destination.write(chunk)
                    destination.flush()
                elif stop.is_set():
                    break
                else:
                    time.sleep(0.01)

    readers = [
        threading.Thread(target=pump, args=(stdout, sys.stdout.buffer), daemon=True),
        threading.Thread(target=pump, args=(stderr, sys.stderr.buffer), daemon=True),
    ]
    for reader in readers:
        reader.start()

    pending = []

    def forward(number, _frame):
        try:
            child_pid = int(pid_file.read_text())
        except (FileNotFoundError, ValueError):
            pending.append(number)
            return
        try:
            os.kill(child_pid, number)
        except ProcessLookupError:
            pass

    signal.signal(signal.SIGINT, forward)
    signal.signal(signal.SIGTERM, forward)

    def pump_input():
        try:
            with open(stdin_file, "wb") as writer:
                while chunk := sys.stdin.buffer.read1(4096):
                    writer.write(chunk)
                    writer.flush()
        except BrokenPipeError:
            pass

    if stdin_file is not None:
        threading.Thread(target=pump_input, daemon=True).start()
    send_request(args.socket, args.pane_id, request_file)
    while not exit_file.exists():
        if pending:
            numbers, pending[:] = pending[:], []
            for number in numbers:
                forward(number, None)
        time.sleep(0.01)
    stop.set()
    for reader in readers:
        reader.join()
    status = json.loads(exit_file.read_text())
    if "code" in status:
        return status["code"]
    signal.signal(status["signal"], signal.SIG_DFL)
    os.kill(os.getpid(), status["signal"])
    return 128 + status["signal"]


def run_script(args):
    request_dir = Path(tempfile.mkdtemp(prefix="script-", dir=args.root))
    request_file = request_dir / "request.json"
    env = allowed_env()
    for key in ("COLLAB_BINARY", "PROJECT_ROOT", "PANE_STDOUT", "PANE_STDERR"):
        env[key] = os.environ[key]
    request = {
        "kind": "script",
        "script": args.script,
        "cwd": args.root,
        "env": env,
        "stdout": env["PANE_STDOUT"],
        "stderr": env["PANE_STDERR"],
    }
    request_file.write_text(json.dumps(request))
    send_request(args.socket, args.pane_id, request_file)
    return 0


def run_runner(args):
    root = Path(args.root)
    ready = ready_path(root, args.pane_id)
    children = set()
    child_lock = threading.Lock()
    if sys.stdin.isatty():
        # A TUI consumes long notification input without the canonical line
        # limit. Keep echo for the existing visible tmux delivery assertions.
        attributes = termios.tcgetattr(sys.stdin.fileno())
        attributes[3] &= ~termios.ICANON
        attributes[6][termios.VMIN] = 1
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(sys.stdin.fileno(), termios.TCSANOW, attributes)

    def remember(process):
        with child_lock:
            children.add(process)

    def execute(request_file):
        request_path = Path(request_file)
        if not request_path.is_relative_to(root):
            return
        try:
            request = json.loads(request_path.read_text())
            if request["kind"] == "script":
                with open(request["stdout"], "ab", buffering=0) as stdout, open(
                    request["stderr"], "ab", buffering=0
                ) as stderr:
                    process = subprocess.Popen(
                        ["/bin/sh", request["script"]],
                        cwd=request["cwd"],
                        env=request["env"],
                        stdin=subprocess.DEVNULL,
                        stdout=stdout,
                        stderr=stderr,
                    )
                    remember(process)
                return
            with open(request.get("stdin") or os.devnull, "rb", buffering=0) as stdin, open(request["stdout"], "wb", buffering=0) as stdout, open(
                request["stderr"], "wb", buffering=0
            ) as stderr:
                process = subprocess.Popen(
                    request["argv"],
                    cwd=request["cwd"],
                    env=request["env"],
                    stdin=stdin,
                    stdout=stdout,
                    stderr=stderr,
                )
                remember(process)
                write_atomic(request["pid_file"], str(process.pid))
                while process.poll() is None:
                    try:
                        os.kill(request["requester_pid"], 0)
                    except ProcessLookupError:
                        process.send_signal(signal.SIGINT)
                        try:
                            process.wait(timeout=1)
                        except subprocess.TimeoutExpired:
                            process.terminate()
                        break
                    time.sleep(0.02)
                code = process.wait()
                result = {"code": code} if code >= 0 else {"signal": -code}
                write_atomic(request["exit_file"], json.dumps(result))
        except Exception as error:
            if request_path.exists():
                request = json.loads(request_path.read_text())
                if "exit_file" in request:
                    write_atomic(request["exit_file"], json.dumps({"code": 127}))
            print(f"pane runner request failed: {error}", file=sys.stderr, flush=True)

    if args.initial_command:
        initial = subprocess.Popen(
            ["/bin/sh", "-c", args.initial_command],
            cwd=root,
            env=allowed_env(),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        remember(initial)
    ready.write_text(str(os.getpid()))
    try:
        for line in sys.stdin:
            match = re.search(re.escape(MARKER) + r"(\S+)", line)
            if match:
                threading.Thread(target=execute, args=(match.group(1),), daemon=True).start()
    finally:
        ready.unlink(missing_ok=True)
        with child_lock:
            for process in children:
                if process.poll() is None:
                    process.terminate()
    return 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["client", "script", "runner"])
    parser.add_argument("--socket")
    parser.add_argument("--pane-id")
    parser.add_argument("--root")
    parser.add_argument("--binary")
    parser.add_argument("--script")
    parser.add_argument("--initial-command")
    parser.add_argument("--pipe-stdin", action="store_true")
    args, remainder = parser.parse_known_args()
    args.args = remainder
    if args.mode == "client":
        return run_client(args)
    if args.mode == "script":
        return run_script(args)
    return run_runner(args)


if __name__ == "__main__":
    raise SystemExit(main())
