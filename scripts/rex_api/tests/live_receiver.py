"""Live harness that owns exactly one Rex receiver session."""

from __future__ import annotations

import argparse
import base64
import json
import os
import subprocess
import sys
import threading
import time
import uuid
from dataclasses import asdict
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlparse

from scripts.rex_api import client
from scripts.rex_api.tests import receiver


SESSION_PREFIX = "rex-api-foundations-20261108-receiver-"
REQUEST_TIMEOUT_S = 10.0
CASE_TEXT = (
    "  中文 😀 e\u0301 引号\"quote\'\'\n"
    "second line with trailing spaces  \n"
    "\n"
)


class Evidence:
    def __init__(self, directory: Path) -> None:
        self.directory = directory
        self.directory.mkdir(parents=True, exist_ok=True)
        self.file = self.directory / "result.jsonl"
        self.file.write_text("", encoding="utf-8")

    def append(self, kind: str, **fields: Any) -> None:
        with self.file.open("a", encoding="utf-8") as handle:
            handle.write(
                json.dumps(
                    {"kind": kind, **fields}, ensure_ascii=False, sort_keys=True
                )
                + "\n"
            )
            handle.flush()


def owned_target(owned: client.OwnedSession) -> client.RexTarget:
    return client.RexTarget(
        endpoint=owned.endpoint,
        session_id=owned.session_id,
        block_id=owned.block_id,
    )


class ReadinessMarkers:
    """Track per-attempt readiness tokens registered before each send."""

    def __init__(self, base_token: str) -> None:
        if not base_token:
            raise client.ValidationError("readiness base token must be non-empty")
        self._base = base_token
        self._lock = threading.Lock()
        self._sent: set[str] = set()
        self._matched: str | None = None

    def next_token(self, index: int) -> str:
        return f"{self._base}-{index}"

    def register(self, token: str) -> None:
        if not token:
            raise client.ValidationError("readiness token must be non-empty")
        with self._lock:
            self._sent.add(token)

    def observe(self, message: object) -> bool:
        if not isinstance(message, str):
            return False
        with self._lock:
            if message in self._sent:
                self._matched = message
                return True
        return False

    @property
    def matched(self) -> str | None:
        with self._lock:
            return self._matched

    @property
    def sent_tokens(self) -> tuple[str, ...]:
        with self._lock:
            return tuple(sorted(self._sent))


def receiver_launch(
    *,
    remote: bool,
    receiver_argv_json: str | None,
    receiver_cwd: str | None,
    run_id: str,
    local_cwd: str | None = None,
) -> tuple[list[str], str]:
    """Build the receiver command without leaking local paths into remote mode."""

    if remote or receiver_argv_json is not None or receiver_cwd is not None:
        return _explicit_receiver_launch(receiver_argv_json, receiver_cwd, run_id)
    if not local_cwd:
        raise client.ValidationError("local receiver cwd must be non-empty")
    return (
        [
            sys.executable,
            "-u",
            str(Path(receiver.__file__).resolve()),
            "--run-id",
            run_id,
        ],
        str(Path(local_cwd).resolve()),
    )


def _explicit_receiver_launch(
    receiver_argv_json: str | None, receiver_cwd: str | None, run_id: str
) -> tuple[list[str], str]:
    if not receiver_argv_json:
        raise client.ValidationError("remote receiver argv JSON is required")
    if not receiver_cwd:
        raise client.ValidationError("remote receiver cwd is required")
    try:
        parsed = json.loads(receiver_argv_json)
    except json.JSONDecodeError as failure:
        raise client.ValidationError(
            f"receiver argv is not valid JSON: {failure}"
        ) from failure
    if (
        not isinstance(parsed, list)
        or not parsed
        or not all(isinstance(item, str) and item for item in parsed)
    ):
        raise client.ValidationError(
            "receiver argv must be a non-empty JSON array of strings"
        )
    command = [run_id if item == "{run_id}" else item for item in parsed]
    return command, str(receiver_cwd)


def _frame(case_id: str, payload: bytes) -> bytes:
    return (
        receiver.FRAME_PREFIX
        + case_id.encode("utf-8")
        + b"\t"
        + str(len(payload)).encode("ascii")
        + b"\n"
        + payload
        + receiver.FRAME_SUFFIX
    )


def _documents(capture: client.CallResult) -> list[dict[str, Any]]:
    if capture.outcome != "completed" or not isinstance(capture.value, dict):
        return []
    documents: list[dict[str, Any]] = []
    for line in str(capture.value.get("text", "")).splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(item, dict):
            documents.append(item)
    return documents


def verify_long_output(captured: str, line_count: int) -> None:
    for index in range(1, line_count + 1):
        marker = f"REXAPI-LONG-{index:08d}-"
        if marker not in captured:
            raise AssertionError(f"long capture missing marker: {marker}")


def _capture_text(
    rex: client.RexClient, target: client.RexTarget, timeout_s: float = 10
) -> str:
    capture = rex.capture(target, timeout_s)
    if capture.outcome != "completed" or not isinstance(capture.value, dict):
        raise RuntimeError(f"capture failed: {capture}")
    return str(capture.value.get("text", ""))


def _wait_for_document(
    rex: client.RexClient,
    target: client.RexTarget,
    predicate,
    deadline: float,
) -> dict[str, Any]:
    while time.monotonic() < deadline:
        for item in _documents(rex.capture(target, REQUEST_TIMEOUT_S)):
            if predicate(item):
                return item
        time.sleep(0.1)
    raise TimeoutError("expected receiver document did not arrive")


def _write_protocol(
    rex: client.RexClient, owned: client.OwnedSession, case_id: str, payload: bytes
) -> None:
    target = owned_target(owned)
    text = _frame(case_id, payload).decode("utf-8")
    receipt = rex.submit(target, text, enter=False, timeout_s=REQUEST_TIMEOUT_S)
    if receipt.acceptance != "queued":
        raise RuntimeError(f"protocol frame not queued: {receipt}")


def _send_frame_ack(
    rex: client.RexClient,
    owned: client.OwnedSession,
    case_id: str,
    payload: bytes,
    evidence: Evidence,
) -> dict[str, Any]:
    _write_protocol(rex, owned, case_id, payload)
    target = owned_target(owned)

    def matches(item: dict[str, Any]) -> bool:
        return item.get("kind") == "test_receiver_ack" and item.get("case_id") == case_id

    item = _wait_for_document(rex, target, matches, time.monotonic() + 10)
    received = base64.b64decode(item["received_base64"])
    if received != payload:
        raise AssertionError(
            f"ACK bytes mismatch for {case_id}: {len(received)} != {len(payload)}"
        )
    evidence.append(
        "receiver_ack",
        case_id=case_id,
        expected_bytes=len(payload),
        received_bytes=len(received),
        exact_match=True,
    )
    return item


def _wait_for_ready(
    rex: client.RexClient, owned: client.OwnedSession, evidence: Evidence
) -> None:
    target = owned_target(owned)
    item = _wait_for_document(
        rex,
        target,
        lambda value: value.get("kind") == "test_receiver_ready",
        time.monotonic() + 20,
    )
    evidence.append("receiver_ready", run_id=item.get("run_id"))


def _process_receiver(
    process_value: dict[str, Any],
    expected_cwd: str,
    *,
    resolve: bool,
    expected_script: str | None = None,
    expected_argv0: str | None = None,
) -> bool:
    def same(actual: str, expected: str) -> bool:
        if resolve:
            return Path(actual).resolve() == Path(expected).resolve()
        return actual == expected

    for key in ("child", "foreground"):
        item = process_value.get(key)
        if not isinstance(item, dict):
            continue
        script = item.get("script")
        argv0 = item.get("argv0")
        cwd = item.get("cwd")
        if not isinstance(cwd, str) or not same(cwd, expected_cwd):
            continue
        if expected_script is not None and (
            not isinstance(script, str) or not same(script, expected_script)
        ):
            continue
        if expected_argv0 is not None and (
            not isinstance(argv0, str) or not same(argv0, expected_argv0)
        ):
            continue
        return True
    return False


def _pwd_matches(
    value: dict[str, Any], expected_cwd: str, *, resolve: bool
) -> bool:
    reported = value.get("pwd")
    if not isinstance(reported, str) or not reported:
        return False
    if not resolve:
        parsed = urlparse(reported)
        return reported == expected_cwd or (
            parsed.scheme == "file" and unquote(parsed.path) == expected_cwd
        )
    expected = Path(expected_cwd).resolve()
    if reported == str(expected):
        return True
    parsed = urlparse(reported)
    return parsed.scheme == "file" and Path(unquote(parsed.path)) == expected


def _status_record(
    rex: client.RexClient,
    owned: client.OwnedSession,
    *,
    record_id: str,
    state: str,
) -> dict[str, Any]:
    result = rex.program_status(owned_target(owned), REQUEST_TIMEOUT_S)
    if result.outcome != "completed" or not isinstance(result.value, dict):
        raise AssertionError(f"program_status failed: {result}")
    for record in result.value.get("records") or []:
        if record.get("id") == record_id and record.get("state") == state:
            return record
    raise AssertionError(
        f"status record {record_id!r}/{state!r} not found: {result.value}"
    )


def _event_payload(
    event: dict[str, Any], target: client.RexTarget
) -> tuple[str, dict[str, Any]] | None:
    return client.event_name_and_payload(event, target)


def observe_status_changes(
    rex: client.RexClient,
    owned: client.OwnedSession,
    evidence: Evidence,
    *,
    attach_grace_s: float = 2.0,
    marker_window_s: float = 12.0,
    marker_step_wait_s: float = 0.5,
    join_timeout_s: float = 15.0,
    cancel_join_timeout_s: float = 3.0,
) -> list[dict[str, Any]]:
    markers = ReadinessMarkers(f"ready-{uuid.uuid4().hex}")
    cancel = threading.Event()
    marker_seen = threading.Event()
    holder: dict[str, client.EventBatch] = {}
    target = owned_target(owned)

    def on_event(event: dict[str, Any]) -> None:
        named = client.event_name_and_payload(event, target)
        if named is None or named[0] != "program_status_changed":
            return
        record = named[1].get("record") or {}
        if markers.observe(record.get("msg")):
            marker_seen.set()

    def observe() -> None:
        duration_s = max(25, REQUEST_TIMEOUT_S * 4)
        holder["batch"] = rex.observe(target, duration_s, cancel, on_event=on_event)

    thread = threading.Thread(target=observe, daemon=True)
    thread.start()
    index = 0
    matched_token: str | None = None
    try:
        # Give the events CLI time to attach before the first marker.
        if attach_grace_s:
            time.sleep(attach_grace_s)
        marker_started = time.monotonic()
        while thread.is_alive() and time.monotonic() - marker_started < marker_window_s:
            attempt = markers.next_token(index)
            # Register before the write so a fast callback can never miss the token.
            markers.register(attempt)
            _write_protocol(rex, owned, f"ready:{attempt}", attempt.encode("utf-8"))
            index += 1
            if marker_seen.wait(timeout=marker_step_wait_s):
                break
        if not marker_seen.is_set():
            raise TimeoutError(
                "readiness marker was not observed in the event stream"
            )
        matched_token = markers.matched
        if matched_token is None:
            raise AssertionError("readiness marker set without a matched token")
        _write_protocol(rex, owned, "status-working", b"")
        _status_record(rex, owned, record_id="test", state="working")
        _write_protocol(rex, owned, "status-blocked", b"")
        _status_record(rex, owned, record_id="test", state="blocked")
        thread.join(timeout=join_timeout_s)
    finally:
        cancel.set()
        thread.join(timeout=cancel_join_timeout_s)
        _save_observation(
            evidence, holder.get("batch"), thread, matched_token, index, markers
        )

    batch = holder.get("batch")
    if batch is None:
        raise TimeoutError("event observation did not return")

    matched: list[dict[str, Any]] = []
    for event in batch.events:
        named = _event_payload(event, target)
        if named is None:
            continue
        name, _payload = named
        if name == "program_status_changed":
            matched.append(event)
    readiness_index = None
    for position, event in enumerate(matched):
        _name, payload = _event_payload(event, target) or ("", {})
        record = payload.get("record") or {}
        if record.get("msg") == matched_token:
            readiness_index = position
            break
    if readiness_index is None:
        raise AssertionError("readiness marker was not observed in the event stream")
    after_readiness = matched[readiness_index + 1 :]
    states = [
        ((_event_payload(event, target) or ("", {}))[1].get("record") or {}).get(
            "state"
        )
        for event in after_readiness
    ]
    if "working" not in states or "blocked" not in states:
        raise AssertionError(f"controlled state changes missing from events: {states}")
    return matched


def _save_observation(
    evidence: Evidence,
    batch: client.EventBatch | None,
    thread: threading.Thread,
    matched_token: str | None,
    attempts: int,
    markers: ReadinessMarkers,
) -> None:
    """Persist the returned batch on both success and readiness failure."""

    if batch is None:
        evidence.append(
            "event_batch_missing",
            thread_alive=thread.is_alive(),
            readiness_attempts=attempts,
            sent_tokens=len(markers.sent_tokens),
        )
        return
    evidence.append(
        "event_batch",
        completion=batch.completion,
        gap_possible=batch.gap_possible,
        event_count=len(batch.events),
        error=batch.error,
        matched_token=matched_token,
        readiness_attempts=attempts,
        sent_tokens=len(markers.sent_tokens),
    )
    with (evidence.directory / "events.jsonl").open(
        "w", encoding="utf-8"
    ) as handle:
        for event in batch.events:
            handle.write(
                json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n"
            )


def _preflight() -> None:
    root = Path(__file__).resolve().parents[3]
    completed = subprocess.run(
        ["git", "-C", str(root), "status", "--short", "--branch"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        shell=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"git status failed: {completed.stderr}")
    if "codex/rex-api-foundations-20261108" not in completed.stdout:
        raise RuntimeError("not running from the task worktree branch")


def _teardown(
    rex: client.RexClient,
    owned: client.OwnedSession | None,
    evidence: Evidence,
) -> dict[str, Any]:
    if owned is None:
        evidence.append("cleanup_noop", reason="no creation receipt")
        return {"closed_session_id": None, "cleaned": False}
    try:
        call = rex.destroy_owned_session(owned, REQUEST_TIMEOUT_S)
    except Exception as failure:
        evidence.append("cleanup_failed", error=str(failure))
        return {
            "closed_session_id": owned.session_id,
            "cleaned": False,
            "error": str(failure),
        }
    if call.outcome != "completed":
        evidence.append("cleanup_failed", error=call.error)
        return {
            "closed_session_id": owned.session_id,
            "cleaned": False,
            "error": call.error,
        }
    exists = rex.session_exists(owned.session_id, REQUEST_TIMEOUT_S)
    evidence.append(
        "cleanup_done",
        session_id=owned.session_id,
        exists_after_close=exists,
    )
    return {"closed_session_id": owned.session_id, "cleaned": not exists}


def _target_json(target: client.RexTarget) -> dict[str, str]:
    return {
        "cli_path": target.endpoint.cli_path,
        "server_address": target.endpoint.server_address,
        "session_id": target.session_id,
        "block_id": target.block_id,
    }


def main(argv: list[str] | None = None) -> int:
    global REQUEST_TIMEOUT_S
    parser = argparse.ArgumentParser(description="Rex API live receiver harness")
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--evidence-dir", required=True)
    parser.add_argument("--cli-path", default=client.DEFAULT_CLI_PATH)
    parser.add_argument("--cwd", default=os.getcwd())
    parser.add_argument("--receiver-argv", default=None)
    parser.add_argument("--receiver-cwd", default=None)
    parser.add_argument(
        "--request-timeout",
        type=float,
        default=10,
        help="finite timeout in seconds for ordinary remote API calls",
    )
    parser.add_argument(
        "--bulk-timeout",
        type=float,
        default=10,
        help="seconds allowed for schema discovery and the full long-output capture",
    )
    args = parser.parse_args(argv)

    evidence = Evidence(Path(args.evidence_dir))
    run_id = uuid.uuid4().hex[:8]
    label = SESSION_PREFIX + run_id
    endpoint = client.RexEndpoint(cli_path=args.cli_path, server_address=args.endpoint)
    rex: client.RexClient | None = None
    owned: client.OwnedSession | None = None
    cleanup: dict[str, Any] | None = None
    try:
        _preflight()
        REQUEST_TIMEOUT_S = client.validate_timeout(args.request_timeout)
        rex = client.RexClient(endpoint)
        capabilities = rex.discover(args.bulk_timeout)
        evidence.append(
            "preflight",
            cli_version=capabilities.cli_version,
            server_version=capabilities.server_version,
            api_revision=capabilities.api_revision,
            operations=capabilities.supported_operations,
        )
        rex.require(
            capabilities,
            "session.create",
            "session.destroy",
            "write",
            "process",
            "pwd",
            "program_status",
            "capture",
        )

        remote = (
            args.receiver_argv is not None
            or args.receiver_cwd is not None
            or not args.endpoint.startswith("unix://")
        )
        command, cwd = receiver_launch(
            remote=remote,
            receiver_argv_json=args.receiver_argv,
            receiver_cwd=args.receiver_cwd,
            run_id=run_id,
            local_cwd=args.cwd,
        )
        evidence.append(
            "receiver_launch",
            remote=remote,
            command=command,
            cwd=cwd,
        )
        owned = rex.create_owned_session(
            label=label,
            command=command,
            cwd=cwd,
            timeout_s=REQUEST_TIMEOUT_S,
        )
        evidence.append(
            "created_owned_session",
            session_id=owned.session_id,
            block_id=owned.block_id,
            creation=asdict(owned),
        )
        target = rex.resolve_target(owned_target(owned), REQUEST_TIMEOUT_S)
        evidence.append("resolved_owned_target", resolved=_target_json(target))
        _wait_for_ready(rex, owned, evidence)

        process_result = rex.process(target, REQUEST_TIMEOUT_S)
        pwd_result = rex.pwd(target, REQUEST_TIMEOUT_S)
        status_result = rex.program_status(target, REQUEST_TIMEOUT_S)
        process_matches = (
            _process_receiver(
                process_result.value,
                cwd,
                resolve=not remote,
                expected_script=str(Path(receiver.__file__).resolve())
                if not remote
                else None,
                expected_argv0=command[0] if remote else None,
            )
            if isinstance(process_result.value, dict)
            else False
        )
        if (
            process_result.outcome != "completed"
            or not isinstance(process_result.value, dict)
            or not process_matches
        ):
            raise AssertionError(f"receiver process not found: {process_result}")
        evidence.append(
            "process_observation",
            child=process_result.value.get("child"),
            foreground=process_result.value.get("foreground"),
        )
        if (
            pwd_result.outcome != "completed"
            or not isinstance(pwd_result.value, dict)
            or not _pwd_matches(pwd_result.value, cwd, resolve=not remote)
        ):
            raise AssertionError(f"reported cwd mismatch: {pwd_result}")
        if (
            status_result.outcome != "completed"
            or not isinstance(status_result.value, dict)
            or status_result.value.get("records") != []
        ):
            raise AssertionError(f"initial program status was not empty: {status_result}")

        _send_frame_ack(rex, owned, "unicode", CASE_TEXT.encode("utf-8"), evidence)
        observe_status_changes(rex, owned, evidence)

        enter_result = rex.press_enter(target, REQUEST_TIMEOUT_S)
        if enter_result.acceptance != "queued":
            raise AssertionError(f"Enter was not accepted: {enter_result}")
        enter_item = _wait_for_document(
            rex,
            target,
            lambda item: item.get("kind") == "test_receiver_raw_input",
            time.monotonic() + 10,
        )
        evidence.append(
            "enter_raw_input",
            received_base64=enter_item.get("received_base64"),
            byte_count=enter_item.get("byte_count"),
        )

        line_count = 9000
        _send_frame_ack(
            rex,
            owned,
            "long-output",
            str(line_count).encode("ascii"),
            evidence,
        )
        required = [
            f"REXAPI-LONG-{index:08d}-"
            for index in (1, line_count // 2, line_count)
        ]
        deadline = time.monotonic() + 15
        captured = ""
        while time.monotonic() < deadline:
            captured = _capture_text(rex, target, args.bulk_timeout)
            if required[-1] in captured:
                break
            time.sleep(0.1)
        verify_long_output(captured, line_count)
        evidence.append(
            "long_capture",
            required_markers=required,
            checked_lines=line_count,
            captured_bytes=len(captured.encode("utf-8")),
        )

        cleanup = _teardown(rex, owned, evidence)
        evidence.append("cleanup_summary", **cleanup)
        print(json.dumps({"outcome": "pass", "cleanup": cleanup}, sort_keys=True))
        return 0 if cleanup.get("cleaned") else 1
    except Exception as failure:
        evidence.append(
            "harness_failed",
            error=str(failure),
            error_kind=type(failure).__name__,
        )
        if rex is not None and owned is not None and cleanup is None:
            try:
                cleanup = _teardown(rex, owned, evidence)
                evidence.append("cleanup_summary", **cleanup)
            except Exception as teardown_failure:
                evidence.append("teardown_failed", error=str(teardown_failure))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
