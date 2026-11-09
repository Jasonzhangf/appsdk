"""JSON CLI for the Rex API foundation module."""

from __future__ import annotations

import argparse
import dataclasses
import json
import signal
import sys
import threading
from typing import Any

from scripts.rex_api import client


def _jsonable(value: Any) -> Any:
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        return {
            field.name: _jsonable(getattr(value, field.name))
            for field in dataclasses.fields(value)
        }
    if isinstance(value, (list, tuple)):
        return [_jsonable(item) for item in value]
    if isinstance(value, dict):
        return {str(key): _jsonable(item) for key, item in value.items()}
    if (
        isinstance(value, str)
        or value is None
        or isinstance(value, bool)
        or isinstance(value, int)
        or isinstance(value, float)
    ):
        return value
    return str(value)


def _target(args) -> client.RexTarget:
    if not args.session or not args.block:
        raise client.ValidationError("--session and --block are required")
    endpoint = client.RexEndpoint(cli_path=args.cli_path, server_address=args.endpoint)
    return client.RexTarget(
        endpoint=endpoint, session_id=args.session, block_id=args.block
    )


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Rex API foundation CLI")
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--cli-path", default=client.DEFAULT_CLI_PATH)
    parser.add_argument("--session")
    parser.add_argument("--block")
    parser.add_argument("--timeout", type=float, default=10.0)
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("discover")
    subparsers.add_parser("server-status")
    subparsers.add_parser("list-hosts")
    subparsers.add_parser("resolve")
    subparsers.add_parser("inspect")
    subparsers.add_parser("capture")
    subparsers.add_parser("process")
    subparsers.add_parser("pwd")
    subparsers.add_parser("program-status")

    submit = subparsers.add_parser("submit")
    submit.add_argument("--enter", action="store_true")
    submit.add_argument("--timeout", type=float, default=argparse.SUPPRESS)

    observe = subparsers.add_parser("observe")
    observe.add_argument("--duration", type=float, required=True)

    return parser


def _emit(value: Any) -> None:
    json.dump(_jsonable(value), sys.stdout, ensure_ascii=True, sort_keys=True)
    sys.stdout.write("\n")
    sys.stdout.flush()


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    try:
        if args.command == "discover":
            endpoint = client.RexEndpoint(
                cli_path=args.cli_path, server_address=args.endpoint
            )
            capabilities = client.discover(endpoint, args.timeout)
            _emit(capabilities)
            return 0
        if args.command in {"server-status", "list-hosts"}:
            endpoint = client.RexEndpoint(
                cli_path=args.cli_path, server_address=args.endpoint
            )
            result = (
                client.server_status(endpoint, args.timeout)
                if args.command == "server-status"
                else client.list_hosts(endpoint, args.timeout)
            )
            _emit(result)
            return 0 if result.outcome == "completed" else 4

        target = _target(args)
        if args.command == "resolve":
            _emit(client.resolve_target(target, args.timeout))
            return 0
        if args.command == "inspect":
            result = client.inspect(target, args.timeout)
            _emit(result)
            return 0 if result.outcome == "completed" else 4
        if args.command == "capture":
            result = client.capture(target, args.timeout)
            _emit(result)
            return 0 if result.outcome == "completed" else 4
        if args.command == "process":
            result = client.process(target, args.timeout)
            _emit(result)
            return 0 if result.outcome == "completed" else 4
        if args.command == "pwd":
            result = client.pwd(target, args.timeout)
            _emit(result)
            return 0 if result.outcome == "completed" else 4
        if args.command == "program-status":
            result = client.program_status(target, args.timeout)
            _emit(result)
            return 0 if result.outcome == "completed" else 4
        if args.command == "submit":
            text = client.read_submit_text(sys.stdin)
            receipt = client.submit(
                target, text, enter=args.enter, timeout_s=args.timeout
            )
            _emit(receipt)
            return 0 if receipt.acceptance == "queued" else 4
        if args.command == "observe":
            cancel = threading.Event()

            def _cancel(_signum, _frame):
                cancel.set()

            previous = signal.signal(signal.SIGINT, _cancel)
            try:
                batch = client.observe(target, args.duration, cancel)
            finally:
                signal.signal(signal.SIGINT, previous)
            _emit(batch)
            if batch.completion == "cancelled":
                return 130
            return 0 if batch.completion == "deadline" else 4
        raise client.ValidationError(f"unsupported command: {args.command}")
    except client.ValidationError as error:
        print(f"validation error: {error}", file=sys.stderr)
        return 2
    except client.CapabilityError as error:
        print(f"capability error: {error}", file=sys.stderr)
        return 3
    except client.RexApiError as error:
        print(f"REX API error: {error}", file=sys.stderr)
        return 4
    except KeyboardInterrupt:
        print("cancelled", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
