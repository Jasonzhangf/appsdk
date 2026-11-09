"""Small Rex client built on the official CLI."""

from __future__ import annotations

import json
import math
import os
import queue
import subprocess
import threading
import time
import uuid
from dataclasses import dataclass
from typing import Any, Iterable, TextIO


DEFAULT_CLI_PATH = (
    "/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex"
)
TERMINAL_CREATOR = "com.superlogical.terminal"
TERMINAL_SHELL_FLAVOR = "com.superlogical.terminal.shell"
NATIVE_REJECTION_CODES = {
    "bad_request",
    "block_not_found",
    "conflict",
    "invalid_request",
    "method_not_found",
    "not_found",
    "session_not_found",
}


class RexApiError(Exception):
    """Base exception for this module."""


class ValidationError(RexApiError):
    """The request is not an explicit, exact Rex target."""


class CapabilityError(RexApiError):
    """A required native capability is absent."""


class NativeCallError(RexApiError):
    """A native call failed before a typed result could be returned."""


@dataclass(frozen=True)
class RexEndpoint:
    cli_path: str = DEFAULT_CLI_PATH
    server_address: str = ""


@dataclass(frozen=True)
class RexTarget:
    endpoint: RexEndpoint
    session_id: str
    block_id: str


@dataclass(frozen=True)
class Capabilities:
    cli_version: str
    server_version: dict[str, Any]
    api_revision: int | None
    supported_operations: dict[str, bool]
    native_schema: dict[str, Any]


@dataclass(frozen=True)
class ProcessResult:
    argv: tuple[str, ...]
    input_text: str | None
    timeout_s: float | None
    returncode: int | None
    stdout: str
    stderr: str
    error: dict[str, Any] | None = None


@dataclass(frozen=True)
class CallResult:
    outcome: str
    acceptance: str | None
    stage: str
    value: object
    error: dict[str, Any] | None


@dataclass(frozen=True)
class SubmitReceipt:
    target: RexTarget
    text_result: CallResult | None
    enter_result: CallResult | None
    acceptance: str


@dataclass(frozen=True)
class EventBatch:
    target: RexTarget
    events: list[dict[str, Any]]
    completion: str
    gap_possible: bool
    error: dict[str, Any] | None


@dataclass(frozen=True)
class OwnedSession:
    endpoint: RexEndpoint
    session_id: str
    block_id: str
    creation_result: dict[str, Any]


def clean_environment(environ: dict[str, str]) -> dict[str, str]:
    return {key: value for key, value in environ.items() if not key.startswith("REX_")}


def validate_endpoint(endpoint: RexEndpoint) -> RexEndpoint:
    if not isinstance(endpoint, RexEndpoint):
        raise ValidationError("endpoint must be a RexEndpoint")
    if not endpoint.cli_path or not os.path.isabs(endpoint.cli_path):
        raise ValidationError("Rex CLI path must be absolute")
    if not isinstance(endpoint.server_address, str) or not endpoint.server_address.strip():
        raise ValidationError("endpoint must include an explicit non-empty --server address")
    return endpoint


def validate_full_id(kind: str, value: str) -> str:
    if kind not in {"session", "block"}:
        raise ValidationError("ID kind must be session or block")
    if not isinstance(value, str) or not value.startswith(f"{kind}:"):
        raise ValidationError(f"{kind} ID must use the exact {kind}: prefix")
    suffix = value[len(kind) + 1 :]
    try:
        canonical = str(uuid.UUID(suffix))
    except (ValueError, AttributeError) as error:
        raise ValidationError(f"{kind} ID must contain a canonical UUID") from error
    if canonical != suffix:
        raise ValidationError(f"{kind} ID must contain a lower-case canonical UUID")
    return value


def validate_target(target: RexTarget) -> RexTarget:
    if not isinstance(target, RexTarget):
        raise ValidationError("target must be a RexTarget")
    validate_endpoint(target.endpoint)
    validate_full_id("session", target.session_id)
    validate_full_id("block", target.block_id)
    return target


def ensure_owned_target(owned: RexTarget, target: RexTarget) -> RexTarget:
    validate_target(owned)
    validate_target(target)
    if owned != target:
        raise ValidationError("target is not the exact owned endpoint/session/block")
    return target


def validate_timeout(timeout_s: float) -> float:
    if (
        not isinstance(timeout_s, (int, float))
        or not math.isfinite(timeout_s)
        or timeout_s <= 0
    ):
        raise ValidationError("timeout_s must be positive and finite")
    return float(timeout_s)


def empty_program_status() -> dict[str, Any]:
    return {"records": []}


def classify_write_success(text: str, value: Any) -> str:
    if not isinstance(value, dict):
        return "unknown"
    if value.get("bytes_written") == len(text.encode("utf-8")):
        return "queued"
    return "unknown"


def combine_inspect(
    *, process: CallResult, pwd: CallResult, program_status: CallResult
) -> dict[str, dict[str, Any]]:
    return {
        "process": {
            "outcome": process.outcome,
            "value": process.value,
            "error": process.error,
        },
        "pwd": {"outcome": pwd.outcome, "value": pwd.value, "error": pwd.error},
        "program_status": {
            "outcome": program_status.outcome,
            "value": program_status.value,
            "error": program_status.error,
        },
    }


def read_submit_text(stream: TextIO) -> str:
    return stream.read()


def _text(value: bytes | str | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="surrogateescape")
    return value


def _duration(value: float) -> str:
    return f"{value:g}s"


def build_base_argv(
    endpoint: RexEndpoint, timeout_s: float, command: Iterable[str]
) -> list[str]:
    validate_endpoint(endpoint)
    timeout = validate_timeout(timeout_s)
    return [
        endpoint.cli_path,
        "--server",
        endpoint.server_address,
        "--autostart=false",
        "--timeout",
        _duration(timeout),
        *command,
    ]


def parse_event_lines(target: RexTarget, lines: Iterable[str]) -> EventBatch:
    validate_target(target)
    events: list[dict[str, Any]] = []
    error: dict[str, Any] | None = None
    for raw_line in lines:
        line = raw_line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as failure:
            error = {
                "kind": "malformed_event_json",
                "message": str(failure),
                "line": raw_line,
            }
            break
        if not isinstance(event, dict):
            error = {"kind": "malformed_event_json", "line": raw_line}
            break
        if event_matches_target(event, target):
            events.append(event)
    completion = "failed" if error else "disconnected"
    return EventBatch(
        target=target,
        events=events,
        completion=completion,
        gap_possible=error is not None,
        error=error,
    )


def event_matches_target(event: dict[str, Any], target: RexTarget) -> bool:
    try:
        wrapper = event["payload"]
        block_event = wrapper["event"]["payload"]
        return (
            event["type"] == "session_event"
            and wrapper["session_id"] == target.session_id
            and wrapper["event"]["type"] == "block_event"
            and block_event["block_id"] == target.block_id
            and block_event["creator_name"] == TERMINAL_CREATOR
        )
    except (KeyError, TypeError):
        return False


def event_name_and_payload(
    event: dict[str, Any], target: RexTarget
) -> tuple[str, dict[str, Any]] | None:
    if not event_matches_target(event, target):
        return None
    try:
        block_event = event["payload"]["event"]["payload"]
        name = block_event["name"]
        payload = block_event.get("payload", {})
    except (KeyError, TypeError):
        return None
    if not isinstance(name, str) or not isinstance(payload, dict):
        return None
    return name, payload


class OfficialCliRunner:
    """Run one official CLI invocation without a shell."""

    def run(
        self, argv: list[str], *, input_text: str | None = None, timeout_s: float | None = None
    ) -> ProcessResult:
        encoded_input = input_text.encode("utf-8") if input_text is not None else None
        try:
            completed = subprocess.run(
                argv,
                input=encoded_input,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=timeout_s,
                check=False,
                shell=False,
                env=clean_environment(dict(os.environ)),
            )
            return ProcessResult(
                argv=tuple(argv),
                input_text=input_text,
                timeout_s=timeout_s,
                returncode=completed.returncode,
                stdout=_text(completed.stdout),
                stderr=_text(completed.stderr),
            )
        except subprocess.TimeoutExpired as failure:
            return ProcessResult(
                argv=tuple(argv),
                input_text=input_text,
                timeout_s=timeout_s,
                returncode=None,
                stdout=_text(failure.stdout),
                stderr=_text(failure.stderr),
                error={"kind": "timeout", "message": str(failure)},
            )
        except OSError as failure:
            return ProcessResult(
                argv=tuple(argv),
                input_text=input_text,
                timeout_s=timeout_s,
                returncode=None,
                stdout="",
                stderr="",
                error={"kind": "os_error", "message": str(failure)},
            )


class RexClient:
    def __init__(self, endpoint: RexEndpoint, *, runner: Any | None = None) -> None:
        self.endpoint = validate_endpoint(endpoint)
        self.runner = runner or OfficialCliRunner()

    def _validate_target(self, target: RexTarget) -> RexTarget:
        target = validate_target(target)
        if target.endpoint != self.endpoint:
            raise ValidationError("target endpoint does not match client endpoint")
        return target

    def _run(
        self,
        command: Iterable[str],
        *,
        timeout_s: float,
        input_text: str | None = None,
    ) -> ProcessResult:
        timeout = validate_timeout(timeout_s)
        argv = build_base_argv(self.endpoint, timeout, command)
        return self.runner.run(argv, input_text=input_text, timeout_s=timeout)

    def _api_argv(
        self,
        method: str,
        *,
        session_id: str | None = None,
    ) -> list[str]:
        command = ["api", "call"]
        if session_id is not None:
            command.extend(["-s", session_id])
        command.extend([method, "-"])
        return command

    def _block_argv(self, target: RexTarget, method: str) -> list[str]:
        return self._api_argv(method, session_id=target.session_id)

    @staticmethod
    def _native_error(result: ProcessResult) -> dict[str, Any]:
        error: dict[str, Any] = {
            "kind": "native_error",
            "returncode": result.returncode,
            "stderr": result.stderr,
            "stdout": result.stdout,
        }
        try:
            payload = json.loads(result.stdout.strip())
        except (json.JSONDecodeError, AttributeError):
            payload = None
        if isinstance(payload, dict):
            code = payload.get("code") or payload.get("error_code")
            nested = payload.get("error")
            if code is None and isinstance(nested, dict):
                code = nested.get("code")
            if isinstance(code, str):
                error["code"] = code
        return error

    @staticmethod
    def _failure_acceptance(result: ProcessResult) -> str:
        if result.error is not None:
            return "unknown"
        error = RexClient._native_error(result)
        if error.get("code") in NATIVE_REJECTION_CODES:
            return "definitely_not_delivered"
        return "unknown"

    @staticmethod
    def _json_call(
        result: ProcessResult, *, stage: str
    ) -> CallResult:
        if result.error is not None or result.returncode != 0:
            return CallResult(
                outcome="failed",
                acceptance=None,
                stage=stage,
                value=None,
                error=RexClient._native_error(result)
                if result.error is None
                else result.error,
            )
        try:
            value = json.loads(result.stdout)
        except json.JSONDecodeError as failure:
            return CallResult(
                outcome="failed",
                acceptance=None,
                stage=stage,
                value=None,
                error={
                    "kind": "malformed_native_json",
                    "message": str(failure),
                    "stdout": result.stdout,
                },
            )
        return CallResult(
            outcome="completed",
            acceptance=None,
            stage=stage,
            value=value,
            error=None,
        )

    @staticmethod
    def _terminal_methods(api_schema: dict[str, Any]) -> set[str]:
        for block_type in api_schema.get("block_types") or []:
            if block_type.get("creator") == TERMINAL_CREATOR:
                return {
                    method.get("name")
                    for method in block_type.get("methods") or []
                    if isinstance(method.get("name"), str)
                }
        return set()

    def discover(self, timeout_s: float = 5) -> Capabilities:
        timeout = validate_timeout(timeout_s)
        cli_result = self._run(["version"], timeout_s=timeout)
        if cli_result.error is not None or cli_result.returncode != 0:
            raise NativeCallError(json.dumps(self._native_error(cli_result), ensure_ascii=False))

        server_result = self._run(
            self._api_argv("server.version"), input_text="{}", timeout_s=timeout
        )
        server_call = self._json_call(server_result, stage="server.version")
        if server_call.outcome != "completed" or not isinstance(server_call.value, dict):
            raise NativeCallError(json.dumps(server_call.error, ensure_ascii=False))

        methods_result = self._run(
            ["api", "list", "--json"], timeout_s=timeout
        )
        methods_call = self._json_call(methods_result, stage="api.list")
        if methods_call.outcome != "completed" or not isinstance(methods_call.value, dict):
            raise NativeCallError(json.dumps(methods_call.error, ensure_ascii=False))

        creators_result = self._run(
            self._api_argv("block_creator.list"), input_text="{}", timeout_s=timeout
        )
        creators_call = self._json_call(creators_result, stage="block_creator.list")
        if creators_call.outcome != "completed" or not isinstance(
            creators_call.value, dict
        ):
            raise NativeCallError(json.dumps(creators_call.error, ensure_ascii=False))

        api_schema = methods_call.value
        top_level = {
            method.get("name")
            for method in api_schema.get("methods") or []
            if isinstance(method.get("name"), str)
        }
        terminal = self._terminal_methods(api_schema)
        operations = {
            "write": "write" in terminal,
            "process": "process" in terminal,
            "pwd": "pwd" in terminal,
            "program_status": "program_status" in terminal,
            "capture": "format" in terminal,
            "press_enter": True,
            "events": True,
            "session.create": "session.create" in top_level,
            "session.destroy": "session.destroy" in top_level,
            "session.list": "session.list" in top_level,
            "session.list_blocks": "session.list_blocks" in top_level,
        }
        revision = server_call.value.get("api_revision", 0)
        if not isinstance(revision, int):
            revision = None
        return Capabilities(
            cli_version=cli_result.stdout.strip(),
            server_version=server_call.value,
            api_revision=revision,
            supported_operations=operations,
            native_schema={
                "methods": api_schema,
                "creators": creators_call.value,
            },
        )

    def require(self, capabilities: Capabilities, *operations: str) -> None:
        missing = [name for name in operations if not capabilities.supported_operations.get(name)]
        if missing:
            raise CapabilityError(f"missing Rex operations: {', '.join(missing)}")

    def server_status(self, timeout_s: float = 5) -> CallResult:
        timeout = validate_timeout(timeout_s)
        result = self._run(
            self._api_argv("server.status"), input_text="{}", timeout_s=timeout
        )
        return self._json_call(result, stage="server.status")

    def list_hosts(self, timeout_s: float = 5) -> CallResult:
        timeout = validate_timeout(timeout_s)
        result = self._run(
            self._api_argv("hosts.list"), input_text="{}", timeout_s=timeout
        )
        return self._json_call(result, stage="hosts.list")

    def resolve_target(self, target: RexTarget, timeout_s: float = 5) -> RexTarget:
        target = self._validate_target(target)
        timeout = validate_timeout(timeout_s)
        sessions_result = self._run(
            self._api_argv("session.list"), input_text="{}", timeout_s=timeout
        )
        sessions_call = self._json_call(sessions_result, stage="session.list")
        if sessions_call.outcome != "completed" or not isinstance(
            sessions_call.value, dict
        ):
            raise NativeCallError(json.dumps(sessions_call.error, ensure_ascii=False))
        sessions = sessions_call.value.get("sessions")
        if not isinstance(sessions, list) or not any(
            isinstance(item, dict) and item.get("session_id") == target.session_id
            for item in sessions
        ):
            raise ValidationError("exact session ID not found")

        blocks_result = self._run(
            self._api_argv("session.list_blocks", session_id=target.session_id),
            input_text=json.dumps({"session_id": target.session_id}),
            timeout_s=timeout,
        )
        blocks_call = self._json_call(blocks_result, stage="session.list_blocks")
        if blocks_call.outcome != "completed" or not isinstance(blocks_call.value, dict):
            raise NativeCallError(json.dumps(blocks_call.error, ensure_ascii=False))
        blocks = blocks_call.value.get("blocks")
        if not isinstance(blocks, list):
            raise ValidationError("exact block ID not found")
        for item in blocks:
            if (
                isinstance(item, dict)
                and item.get("block_id") == target.block_id
                and item.get("creator_name") == TERMINAL_CREATOR
            ):
                return target
        raise ValidationError("exact terminal block not found in session")

    def _block_call(
        self, target: RexTarget, method: str, args: dict[str, Any], timeout_s: float
    ) -> CallResult:
        target = self._validate_target(target)
        request = {
            "session_id": target.session_id,
            "block_id": target.block_id,
            "args": args,
        }
        result = self._run(
            self._block_argv(target, method),
            input_text=json.dumps(request, ensure_ascii=False),
            timeout_s=timeout_s,
        )
        return self._json_call(result, stage=method)

    def _write_text(self, target: RexTarget, text: str, timeout_s: float) -> CallResult:
        result = self._block_call(
            target, f"{TERMINAL_CREATOR}.write", {"data": text}, timeout_s
        )
        if result.outcome != "completed":
            result = CallResult(
                outcome="failed",
                acceptance="definitely_not_delivered"
                if (
                    result.error
                    and result.error.get("kind") == "native_error"
                    and result.error.get("code") in NATIVE_REJECTION_CODES
                )
                else "unknown",
                stage=result.stage,
                value=None,
                error=result.error,
            )
            return result
        acceptance = classify_write_success(text, result.value)
        if acceptance != "queued":
            return CallResult(
                outcome="failed",
                acceptance="unknown",
                stage=result.stage,
                value=result.value,
                error={
                    "kind": "write_result_mismatch",
                    "expected_bytes": len(text.encode("utf-8")),
                    "value": result.value,
                },
            )
        return CallResult(
            outcome="completed",
            acceptance="queued",
            stage=result.stage,
            value=result.value,
            error=None,
        )

    def write_text(self, target: RexTarget, text: str, timeout_s: float = 5) -> CallResult:
        self._validate_target(target)
        validate_timeout(timeout_s)
        if not isinstance(text, str):
            raise ValidationError("text must be a string")
        return self._write_text(target, text, timeout_s)

    def _press_enter(self, target: RexTarget, timeout_s: float) -> CallResult:
        result = self._run(
            ["send-key", "-s", target.session_id, "-b", target.block_id, "enter"],
            timeout_s=timeout_s,
        )
        if result.error is not None or result.returncode != 0:
            acceptance = self._failure_acceptance(result)
            return CallResult(
                outcome="failed",
                acceptance=acceptance,
                stage="send-key",
                value=None,
                error=self._native_error(result)
                if result.error is None
                else result.error,
            )
        return CallResult(
            outcome="completed",
            acceptance="queued",
            stage="send-key",
            value={"source": "send-key", "key": "enter", "action": "press+release"},
            error=None,
        )

    def press_enter(self, target: RexTarget, timeout_s: float = 5) -> CallResult:
        self._validate_target(target)
        validate_timeout(timeout_s)
        return self._press_enter(target, timeout_s)

    def submit(
        self, target: RexTarget, text: str, enter: bool, timeout_s: float = 5
    ) -> SubmitReceipt:
        target = self._validate_target(target)
        timeout = validate_timeout(timeout_s)
        text_result = self.write_text(target, text, timeout)
        enter_result = None
        if enter and text_result.acceptance == "queued":
            enter_result = self.press_enter(target, timeout)

        if enter_result is None:
            acceptance = text_result.acceptance or "unknown"
        elif (
            text_result.acceptance == "queued"
            and enter_result.acceptance == "queued"
        ):
            acceptance = "queued"
        elif (
            text_result.acceptance == "definitely_not_delivered"
            and enter_result is None
        ):
            acceptance = "definitely_not_delivered"
        else:
            acceptance = "unknown"
        return SubmitReceipt(
            target=target,
            text_result=text_result,
            enter_result=enter_result,
            acceptance=acceptance,
        )

    def capture(self, target: RexTarget, timeout_s: float = 10) -> CallResult:
        target = self._validate_target(target)
        timeout = validate_timeout(timeout_s)
        result = self._run(
            [
                "capture",
                "-s",
                target.session_id,
                "-b",
                target.block_id,
                "--unwrap",
            ],
            timeout_s=timeout,
        )
        if result.error is not None or result.returncode != 0:
            return CallResult(
                outcome="failed",
                acceptance=None,
                stage="capture",
                value={"text": result.stdout, "partial": True},
                error=self._native_error(result)
                if result.error is None
                else result.error,
            )
        return CallResult(
            outcome="completed",
            acceptance=None,
            stage="capture",
            value={"text": result.stdout, "partial": False},
            error=None,
        )

    def process(self, target: RexTarget, timeout_s: float = 5) -> CallResult:
        target = self._validate_target(target)
        timeout = validate_timeout(timeout_s)
        return self._block_call(target, f"{TERMINAL_CREATOR}.process", {}, timeout)

    def pwd(self, target: RexTarget, timeout_s: float = 5) -> CallResult:
        target = self._validate_target(target)
        timeout = validate_timeout(timeout_s)
        return self._block_call(target, f"{TERMINAL_CREATOR}.pwd", {}, timeout)

    def program_status(self, target: RexTarget, timeout_s: float = 5) -> CallResult:
        target = self._validate_target(target)
        timeout = validate_timeout(timeout_s)
        return self._block_call(
            target, f"{TERMINAL_CREATOR}.program_status", {}, timeout
        )

    def inspect(self, target: RexTarget, timeout_s: float = 5) -> CallResult:
        target = self._validate_target(target)
        timeout = validate_timeout(timeout_s)
        process_result = self.process(target, timeout)
        pwd_result = self.pwd(target, timeout)
        status_result = self.program_status(target, timeout)
        value = combine_inspect(
            process=process_result,
            pwd=pwd_result,
            program_status=status_result,
        )
        all_completed = all(
            item["outcome"] == "completed" for item in value.values()
        )
        return CallResult(
            outcome="completed" if all_completed else "failed",
            acceptance=None,
            stage="inspect",
            value=value,
            error=None
            if all_completed
            else {"kind": "partial_inspect", "results": value},
        )

    def _start_events(self, target: RexTarget, timeout_s: float) -> subprocess.Popen[str]:
        command = ["events", "-s", target.session_id, "--json"]
        argv = build_base_argv(self.endpoint, timeout_s, command)
        return subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="surrogateescape",
            shell=False,
            env=clean_environment(dict(os.environ)),
        )

    @staticmethod
    def _terminate_owned_process(process: subprocess.Popen[str]) -> None:
        if process.poll() is not None:
            return
        process.terminate()
        try:
            process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=1)

    def observe(
        self,
        target: RexTarget,
        duration_s: float,
        cancel_event: threading.Event | None = None,
        *,
        on_event=None,
    ) -> EventBatch:
        target = self._validate_target(target)
        duration = validate_timeout(duration_s)
        cancel = cancel_event or threading.Event()
        start_timeout = min(duration, 5.0)
        process = self._start_events(target, start_timeout)
        messages: queue.Queue[tuple[str, str]] = queue.Queue()

        def read_lines(source: str, stream: TextIO | None) -> None:
            if stream is None:
                return
            try:
                for line in stream:
                    messages.put((source, line))
            finally:
                messages.put((source, ""))

        readers = [
            threading.Thread(target=read_lines, args=("stdout", process.stdout), daemon=True),
            threading.Thread(target=read_lines, args=("stderr", process.stderr), daemon=True),
        ]
        for reader in readers:
            reader.start()

        deadline = time.monotonic() + duration
        events: list[dict[str, Any]] = []
        stderr_parts: list[str] = []
        error: dict[str, Any] | None = None
        completion = "deadline"
        stdout_closed = False
        stderr_closed = False
        try:
            while time.monotonic() < deadline:
                if cancel.is_set():
                    completion = "cancelled"
                    break
                wait_s = min(0.1, max(0.0, deadline - time.monotonic()))
                try:
                    source, line = messages.get(timeout=wait_s)
                except queue.Empty:
                    if process.poll() is not None and stdout_closed and stderr_closed:
                        completion = "disconnected"
                        error = {
                            "kind": "events_disconnected",
                            "returncode": process.returncode,
                            "stderr": "".join(stderr_parts),
                        }
                        break
                    continue
                if not line:
                    if source == "stdout":
                        stdout_closed = True
                    else:
                        stderr_closed = True
                    continue
                if source == "stderr":
                    stderr_parts.append(line)
                    continue
                try:
                    event = json.loads(line)
                except json.JSONDecodeError as failure:
                    completion = "failed"
                    error = {
                        "kind": "malformed_event_json",
                        "message": str(failure),
                        "line": line,
                    }
                    break
                if not isinstance(event, dict):
                    completion = "failed"
                    error = {"kind": "malformed_event_json", "line": line}
                    break
                if not event_matches_target(event, target):
                    continue
                events.append(event)
                if on_event is not None:
                    on_event(event)
                if process.poll() is not None and stdout_closed and stderr_closed:
                    completion = "disconnected"
                    break
        finally:
            self._terminate_owned_process(process)
            for reader in readers:
                reader.join(timeout=1)

        if completion == "failed":
            gap_possible = True
        elif completion == "disconnected":
            gap_possible = True
            if error is None:
                error = {
                    "kind": "events_disconnected",
                    "returncode": process.returncode,
                    "stderr": "".join(stderr_parts),
                }
        else:
            gap_possible = False
        return EventBatch(
            target=target,
            events=events,
            completion=completion,
            gap_possible=gap_possible,
            error=error,
        )

    def create_owned_session(
        self,
        *,
        label: str,
        command: list[str],
        cwd: str,
        timeout_s: float = 10,
    ) -> OwnedSession:
        if not label:
            raise ValidationError("session label must be non-empty")
        if not command or not all(isinstance(item, str) and item for item in command):
            raise ValidationError("receiver command must be a non-empty argv list")
        if not os.path.isabs(cwd):
            raise ValidationError("receiver cwd must be absolute")
        request = {
            "label": label,
            "initial_windows": [
                {
                    "window_label": "receiver",
                    "layout": {
                        "block": {
                            "flavor": TERMINAL_SHELL_FLAVOR,
                            "options": {
                                "shell": "none",
                                "login": False,
                                "command": command,
                                "cwd": cwd,
                            },
                        }
                    },
                }
            ],
        }
        result = self._run(
            self._api_argv("session.create"),
            input_text=json.dumps(request, ensure_ascii=False),
            timeout_s=timeout_s,
        )
        call = self._json_call(result, stage="session.create")
        value = call.value if call.outcome == "completed" else None
        try:
            session_id = value["session_id"]
            windows = value["initial_windows"]
            window = windows[0]
            block_id = window["block_ids"][0]
            validate_full_id("session", session_id)
            validate_full_id("block", block_id)
            if len(windows) != 1 or len(window["block_ids"]) != 1:
                raise ValueError("unexpected initial window shape")
        except (TypeError, KeyError, IndexError, ValueError) as failure:
            raise NativeCallError(
                json.dumps(
                    {
                        "kind": "unknown_session_creation",
                        "message": "session.create did not return one exact session/block",
                        "result": value,
                        "error": call.error,
                        "cause": str(failure),
                    },
                    ensure_ascii=False,
                )
            ) from failure
        assert isinstance(value, dict)
        return OwnedSession(
            endpoint=self.endpoint,
            session_id=session_id,
            block_id=block_id,
            creation_result=value,
        )

    def destroy_owned_session(
        self, owned: OwnedSession, timeout_s: float = 10
    ) -> CallResult:
        validate_endpoint(owned.endpoint)
        if owned.endpoint != self.endpoint:
            raise ValidationError("owned session endpoint does not match client endpoint")
        validate_full_id("session", owned.session_id)
        validate_full_id("block", owned.block_id)
        result = self._run(
            self._api_argv("session.destroy", session_id=owned.session_id),
            input_text=json.dumps({"session_id": owned.session_id}),
            timeout_s=timeout_s,
        )
        call = self._json_call(result, stage="session.destroy")
        if call.outcome == "completed":
            return call
        return CallResult(
            outcome="failed",
            acceptance=None,
            stage="session.destroy",
            value=None,
            error=call.error,
        )

    def session_exists(self, session_id: str, timeout_s: float = 5) -> bool:
        validate_full_id("session", session_id)
        result = self._run(
            self._api_argv("session.list"), input_text="{}", timeout_s=timeout_s
        )
        call = self._json_call(result, stage="session.list")
        if call.outcome != "completed" or not isinstance(call.value, dict):
            raise NativeCallError(json.dumps(call.error, ensure_ascii=False))
        sessions = call.value.get("sessions")
        if not isinstance(sessions, list):
            return False
        return any(
            isinstance(item, dict) and item.get("session_id") == session_id
            for item in sessions
        )


def discover(endpoint: RexEndpoint, timeout_s: float = 5) -> Capabilities:
    return RexClient(endpoint).discover(timeout_s)


def server_status(endpoint: RexEndpoint, timeout_s: float = 5) -> CallResult:
    return RexClient(endpoint).server_status(timeout_s)


def list_hosts(endpoint: RexEndpoint, timeout_s: float = 5) -> CallResult:
    return RexClient(endpoint).list_hosts(timeout_s)


def resolve_target(target: RexTarget, timeout_s: float = 5) -> RexTarget:
    return RexClient(target.endpoint).resolve_target(target, timeout_s)


def write_text(target: RexTarget, text: str, timeout_s: float = 5) -> CallResult:
    return RexClient(target.endpoint).write_text(target, text, timeout_s)


def press_enter(target: RexTarget, timeout_s: float = 5) -> CallResult:
    return RexClient(target.endpoint).press_enter(target, timeout_s)


def submit(
    target: RexTarget, text: str, enter: bool, timeout_s: float = 5
) -> SubmitReceipt:
    return RexClient(target.endpoint).submit(target, text, enter, timeout_s)


def capture(target: RexTarget, timeout_s: float = 10) -> CallResult:
    return RexClient(target.endpoint).capture(target, timeout_s)


def process(target: RexTarget, timeout_s: float = 5) -> CallResult:
    return RexClient(target.endpoint).process(target, timeout_s)


def pwd(target: RexTarget, timeout_s: float = 5) -> CallResult:
    return RexClient(target.endpoint).pwd(target, timeout_s)


def program_status(target: RexTarget, timeout_s: float = 5) -> CallResult:
    return RexClient(target.endpoint).program_status(target, timeout_s)


def inspect(target: RexTarget, timeout_s: float = 5) -> CallResult:
    return RexClient(target.endpoint).inspect(target, timeout_s)


def observe(
    target: RexTarget,
    duration_s: float,
    cancel_event: threading.Event | None = None,
    *,
    on_event=None,
) -> EventBatch:
    return RexClient(target.endpoint).observe(
        target, duration_s, cancel_event, on_event=on_event
    )
