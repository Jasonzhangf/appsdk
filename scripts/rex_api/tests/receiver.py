"""Bounded test receiver used only by the Rex live harness."""

from __future__ import annotations

import argparse
import base64
import json
import os
import sys
import termios
import tty
from dataclasses import dataclass
from pathlib import Path


FRAME_PREFIX = b"\x1eREXFRAME:"
FRAME_SUFFIX = b"\x1e"
APP_NAME = "rex-api-foundations"


def _json_line(value: dict[str, object]) -> bytes:
    return (json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n").encode()


def ack_bytes(case_id: str, received: bytes) -> bytes:
    return _json_line(
        {
            "kind": "test_receiver_ack",
            "case_id": case_id,
            "received_base64": base64.b64encode(received).decode("ascii"),
            "byte_count": len(received),
        }
    )


def osc7501(
    state: str,
    *,
    record_id: str = "test",
    app: str = APP_NAME,
    message: str | None = None,
) -> bytes:
    if state not in {"idle", "working", "done", "blocked", "error"}:
        raise ValueError(f"unsupported OSC 7501 state: {state}")
    message_text = message if message is not None else state
    encoded_message = base64.b64encode(message_text.encode("utf-8")).decode("ascii")
    body = (
        f"state={state}:id={record_id}:app={app}:msg={encoded_message}".encode("ascii")
    )
    return b"\x1b]7501;" + body + b"\x1b\\"


def osc7(path: str) -> bytes:
    return b"\x1b]7;" + Path(path).resolve().as_uri().encode("utf-8") + b"\x1b\\"


def long_output(line_count: int) -> bytes:
    if line_count <= 0:
        raise ValueError("line_count must be positive")
    return b"".join(
        f"REXAPI-LONG-{index:08d}-".encode("ascii") + b"x" * 96 + b"\n"
        for index in range(1, line_count + 1)
    )


@dataclass(frozen=True)
class Frame:
    case_id: str
    payload: bytes


@dataclass(frozen=True)
class RawInput:
    payload: bytes


class FrameParser:
    """Parse length-prefixed byte frames without interpreting their payload."""

    def __init__(self) -> None:
        self._buffer = bytearray()

    def feed(self, data: bytes) -> list[Frame | RawInput | str]:
        self._buffer.extend(data)
        frames: list[Frame | RawInput | str] = []
        while self._buffer:
            start = self._buffer.find(FRAME_PREFIX)
            if start < 0:
                partial = self._partial_prefix_length()
                raw = bytes(self._buffer[: len(self._buffer) - partial])
                if raw:
                    frames.append(RawInput(raw))
                    del self._buffer[: len(raw)]
                break
            if start:
                frames.append(RawInput(bytes(self._buffer[:start])))
                del self._buffer[:start]
            newline = self._buffer.find(b"\n", len(FRAME_PREFIX))
            if newline < 0:
                break
            header = bytes(self._buffer[len(FRAME_PREFIX) : newline])
            del self._buffer[: newline + 1]
            try:
                case_text, length_text = header.split(b"\t", 1)
                case_id = case_text.decode("utf-8")
                length = int(length_text)
            except (UnicodeDecodeError, ValueError):
                frames.append("protocol_error")
                continue
            if length < 0:
                frames.append("protocol_error")
                continue
            needed = length + len(FRAME_SUFFIX)
            if len(self._buffer) < needed:
                self._buffer[:0] = FRAME_PREFIX + header + b"\n"
                break
            payload = bytes(self._buffer[:length])
            suffix = bytes(self._buffer[length:needed])
            del self._buffer[:needed]
            if suffix != FRAME_SUFFIX:
                frames.append("protocol_error")
                continue
            frames.append(Frame(case_id=case_id, payload=payload))
        return frames

    def _partial_prefix_length(self) -> int:
        maximum = min(len(self._buffer), len(FRAME_PREFIX) - 1)
        for size in range(maximum, 0, -1):
            if self._buffer[-size:] == FRAME_PREFIX[:size]:
                return size
        return 0


class Receiver:
    def __init__(self, output, run_id: str) -> None:
        self._output = output
        self._run_id = run_id
        self._ready_count = 0

    def _write(self, data: bytes) -> None:
        self._output.buffer.write(data)
        self._output.buffer.flush()

    def ready(self) -> None:
        self._write(osc7(str(Path.cwd())))
        self._write(_json_line({"kind": "test_receiver_ready", "run_id": self._run_id}))

    def handle(self, frame: Frame) -> None:
        self._write(ack_bytes(frame.case_id, frame.payload))
        if frame.case_id == "status-working":
            self._write(osc7501("working"))
        elif frame.case_id == "status-blocked":
            self._write(osc7501("blocked"))
        elif frame.case_id == "status-idle":
            self._write(osc7501("idle"))
        elif frame.case_id.startswith("ready:"):
            token = frame.case_id.split(":", 1)[1]
            state = "working" if self._ready_count % 2 == 0 else "blocked"
            self._ready_count += 1
            self._write(osc7501(state, record_id="ready", message=token))
        elif frame.case_id == "long-output":
            try:
                line_count = int(frame.payload.decode("ascii"))
            except (UnicodeDecodeError, ValueError):
                self._write(_json_line({"kind": "test_receiver_error", "error": "bad_line_count"}))
                return
            self._write(long_output(line_count))

    def raw_input(self, payload: bytes) -> None:
        self._write(
            _json_line(
                {
                    "kind": "test_receiver_raw_input",
                    "received_base64": base64.b64encode(payload).decode("ascii"),
                    "byte_count": len(payload),
                }
            )
        )

    def protocol_error(self) -> None:
        self._write(_json_line({"kind": "test_receiver_error", "error": "protocol_error"}))


def serve(run_id: str, input_stream, output, *, debug_bytes: bool = False) -> int:
    receiver = Receiver(output=output, run_id=run_id)
    parser = FrameParser()
    fd = input_stream.fileno()
    previous = termios.tcgetattr(fd)
    try:
        tty.setraw(fd, termios.TCSANOW)
        receiver.ready()
        while True:
            chunk = os.read(fd, 4096)
            if not chunk:
                return 0
            if debug_bytes:
                receiver._write(
                    _json_line(
                        {
                            "kind": "test_receiver_bytes",
                            "run_id": run_id,
                            "base64": base64.b64encode(chunk).decode("ascii"),
                            "byte_count": len(chunk),
                        }
                    )
                )
            for item in parser.feed(chunk):
                if isinstance(item, RawInput):
                    receiver.raw_input(item.payload)
                elif item == "protocol_error":
                    receiver.protocol_error()
                else:
                    receiver.handle(item)
    finally:
        termios.tcsetattr(fd, termios.TCSANOW, previous)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Rex API test receiver")
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--debug-bytes", action="store_true")
    args = parser.parse_args(argv)
    return serve(
        args.run_id, sys.stdin, sys.stdout, debug_bytes=args.debug_bytes
    )


if __name__ == "__main__":
    raise SystemExit(main())
