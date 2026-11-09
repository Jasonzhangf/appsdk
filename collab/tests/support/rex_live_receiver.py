#!/usr/bin/env python3
"""Test-only receiver for the explicit Rex live contract.

The receiver is deliberately small. It records the exact bytes it reads from
the terminal so the harness can distinguish a real receiver ACK from terminal
echo. It is test data only and is never used by the production adapter.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import sys
import termios
import tty


FRAME_PREFIX = b"\x1eREXFRAME:"
FRAME_SUFFIX = b"\x1e"


def emit(file_handle, value: dict[str, object]) -> None:
    line = json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n"
    file_handle.write(line)
    file_handle.flush()
    sys.stdout.write(line)
    sys.stdout.flush()


def decoded(data: bytes) -> str | None:
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return None


def emit_received(
    file_handle,
    run_id: str,
    kind: str,
    data: bytes,
    *,
    case_id: str | None = None,
) -> None:
    value: dict[str, object] = {
        "kind": kind,
        "run_id": run_id,
        "byte_count": len(data),
        "received_base64": base64.b64encode(data).decode("ascii"),
    }
    text = decoded(data)
    if text is not None:
        value["received"] = text
    if case_id is not None:
        value["case_id"] = case_id
    emit(file_handle, value)


class FrameParser:
    """Separate framed receiver probes from raw terminal input."""

    def __init__(self, file_handle, run_id: str) -> None:
        self._file = file_handle
        self._run_id = run_id
        self._buffer = bytearray()
        self._raw = bytearray()

    def _flush_raw_lines(self) -> None:
        while True:
            positions = [
                position
                for marker in (b"\r", b"\n")
                if (position := self._raw.find(marker)) >= 0
            ]
            if not positions:
                return
            end = min(positions)
            line = bytes(self._raw[:end])
            del self._raw[: end + 1]
            if self._raw[:1] == b"\n":
                del self._raw[:1]
            if line:
                emit_received(
                    self._file,
                    self._run_id,
                    "test_receiver_raw_input",
                    line,
                )

    def _protocol_error(self) -> None:
        emit(
            self._file,
            {"kind": "test_receiver_error", "run_id": self._run_id, "error": "protocol_error"},
        )

    def _partial_prefix_length(self) -> int:
        maximum = min(len(self._buffer), len(FRAME_PREFIX) - 1)
        for size in range(maximum, 0, -1):
            if self._buffer[-size:] == FRAME_PREFIX[:size]:
                return size
        return 0

    def feed(self, data: bytes) -> None:
        self._buffer.extend(data)
        while self._buffer:
            start = self._buffer.find(FRAME_PREFIX)
            if start < 0:
                partial = self._partial_prefix_length()
                keep = len(self._buffer) - partial
                if keep:
                    self._raw.extend(self._buffer[:keep])
                    del self._buffer[:keep]
                    self._flush_raw_lines()
                return
            if start:
                self._raw.extend(self._buffer[:start])
                del self._buffer[:start]
                self._flush_raw_lines()
                continue

            newline = self._buffer.find(b"\n", len(FRAME_PREFIX))
            if newline < 0:
                return
            header = bytes(self._buffer[len(FRAME_PREFIX) : newline])
            try:
                case_bytes, length_bytes = header.split(b"\t", 1)
                case_id = case_bytes.decode("utf-8")
                length = int(length_bytes)
            except (UnicodeDecodeError, ValueError):
                del self._buffer[: newline + 1]
                self._protocol_error()
                continue
            if length < 0:
                del self._buffer[: newline + 1]
                self._protocol_error()
                continue
            payload_start = newline + 1
            payload_end = payload_start + length
            needed = payload_end + len(FRAME_SUFFIX)
            if len(self._buffer) < needed:
                return
            payload = bytes(self._buffer[payload_start:payload_end])
            suffix = bytes(self._buffer[payload_end:needed])
            del self._buffer[:needed]
            if suffix != FRAME_SUFFIX:
                self._protocol_error()
                continue
            emit_received(
                self._file,
                self._run_id,
                "test_receiver_ack",
                payload,
                case_id=case_id,
            )
            if case_id == "osc-status":
                sys.stdout.write("\x1b]7501;" + payload.decode("ascii") + "\x1b\\")
                sys.stdout.flush()

    def finish(self) -> None:
        if self._raw:
            emit_received(
                self._file,
                self._run_id,
                "test_receiver_raw_input",
                bytes(self._raw),
            )
            self._raw.clear()


def serve(args: argparse.Namespace) -> int:
    input_fd = sys.stdin.fileno()
    previous = termios.tcgetattr(input_fd)
    with open(args.ack_file, "w", encoding="utf-8") as ack_file:
        parser = FrameParser(ack_file, args.run_id)
        try:
            tty.setraw(input_fd, termios.TCSANOW)
            emit(
                ack_file,
                {"kind": "test_receiver_ready", "run_id": args.run_id},
            )
            while True:
                chunk = os.read(input_fd, 4096)
                if not chunk:
                    break
                parser.feed(chunk)
            parser.finish()
        finally:
            termios.tcsetattr(input_fd, termios.TCSANOW, previous)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Rex live test receiver")
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--ack-file", required=True)
    return serve(parser.parse_args(argv))


if __name__ == "__main__":
    raise SystemExit(main())
