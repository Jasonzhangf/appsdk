from __future__ import annotations

import base64
import io
import json
import subprocess
import sys
import tempfile
import time
import unittest
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

from scripts.rex_api import client
from scripts.rex_api import __main__ as cli
from scripts.rex_api.tests import live_receiver, receiver


SESSION_ID = "session:018f5f6e-6d1a-7b31-8c20-123456789abc"
BLOCK_ID = "block:018f5f6e-6d1a-7b31-8c20-abcdef012345"
ENDPOINT = client.RexEndpoint(
    cli_path="/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex",
    server_address="unix:///Users/fanzhang/Library/Application%20Support/rex/server.sock",
)
TARGET = client.RexTarget(endpoint=ENDPOINT, session_id=SESSION_ID, block_id=BLOCK_ID)


def completed(value):
    return client.CallResult(
        outcome="completed", acceptance=None, stage="test", value=value, error=None
    )


class FakeRunner:
    def __init__(self, results):
        self.results = list(results)
        self.calls = []

    def run(self, argv, *, input_text=None, timeout_s=None):
        self.calls.append(
            client.ProcessResult(
                argv=tuple(argv),
                input_text=input_text,
                timeout_s=timeout_s,
                returncode=0,
                stdout="",
                stderr="",
            )
        )
        if not self.results:
            raise AssertionError("unexpected runner call")
        return self.results.pop(0)


class IdentityTests(unittest.TestCase):
    def test_full_ids_require_exact_kind_prefix_and_uuid(self):
        self.assertEqual(client.validate_full_id("session", SESSION_ID), SESSION_ID)
        self.assertEqual(client.validate_full_id("block", BLOCK_ID), BLOCK_ID)
        for value in ("work", "018f5f6e", "window:018f5f6e-6d1a-7b31-8c20-123456789abc"):
            with self.subTest(value=value), self.assertRaises(client.ValidationError):
                client.validate_full_id("session", value)

    def test_owned_target_rejects_cross_session_or_block(self):
        other = replace(TARGET, block_id="block:018f5f6e-6d1a-7b31-8c20-fedcba987654")
        client.ensure_owned_target(TARGET, TARGET)
        with self.assertRaises(client.ValidationError):
            client.ensure_owned_target(TARGET, other)


class RunnerTests(unittest.TestCase):
    def test_argv_is_explicit_shell_free_and_environment_is_cleaned(self):
        runner = FakeRunner(
            [
                client.ProcessResult(
                    argv=(),
                    input_text=None,
                    timeout_s=3,
                    returncode=0,
                    stdout="Rex Beta 0.1.0",
                    stderr="",
                ),
                client.ProcessResult(
                    argv=(),
                    input_text="{}",
                    timeout_s=3,
                    returncode=0,
                    stdout='{"rex_version":"0.1.0","api_revision":11,"api_min_revision":11}',
                    stderr="",
                ),
                client.ProcessResult(
                    argv=(),
                    input_text=None,
                    timeout_s=3,
                    returncode=0,
                    stdout='{"methods":[{"name":"session.list"}],"block_types":[{"creator":"com.superlogical.terminal","methods":[{"name":"write"},{"name":"process"},{"name":"pwd"},{"name":"program_status"},{"name":"format"}]}]}',
                    stderr="",
                ),
                client.ProcessResult(
                    argv=(),
                    input_text="{}",
                    timeout_s=3,
                    returncode=0,
                    stdout='{"creators":[]}',
                    stderr="",
                ),
            ]
        )
        client.RexClient(ENDPOINT, runner=runner).discover(timeout_s=3)
        call = runner.calls[0]
        self.assertIn("--autostart=false", call.argv)
        self.assertIn("--server", call.argv)
        self.assertEqual(call.argv[call.argv.index("--server") + 1], ENDPOINT.server_address)
        self.assertNotIn("REX_SESSION_ID", client.clean_environment({}))

    def test_local_validation_does_not_call_runner(self):
        runner = FakeRunner([])
        with self.assertRaises(client.ValidationError):
            client.RexClient(ENDPOINT, runner=runner).write_text(
                replace(TARGET, block_id="block:short"), "x", timeout_s=3
            )
        self.assertEqual(runner.calls, [])


class SubmitTests(unittest.TestCase):
    class Client(client.RexClient):
        def __init__(self):
            super().__init__(ENDPOINT, runner=FakeRunner([]))
            self.writes = []
            self.enters = 0

        def _write_text(self, target, text, timeout_s):
            self.writes.append((target, text, timeout_s))
            return client.CallResult(
                outcome="completed",
                acceptance="queued",
                stage="test_text",
                value={"bytes_written": len(text.encode("utf-8"))},
                error=None,
            )

        def _press_enter(self, target, timeout_s):
            self.enters += 1
            return client.CallResult(
                outcome="cancelled",
                acceptance="unknown",
                stage="test_enter",
                value=None,
                error={"kind": "test_timeout"},
            )

    def test_submit_preserves_text_but_keeps_enter_result_separate(self):
        subject = self.Client()
        text = "  中文 😀 e\u0301\nsecond line  \n"
        receipt = subject.submit(TARGET, text, enter=True, timeout_s=3)
        self.assertEqual(subject.writes, [(TARGET, text, 3)])
        self.assertEqual(subject.enters, 1)
        self.assertEqual(receipt.text_result.acceptance, "queued")
        self.assertEqual(receipt.enter_result.outcome, "cancelled")
        self.assertEqual(receipt.acceptance, "unknown")

    def test_submit_does_not_send_enter_after_unknown_text(self):
        class UnknownTextClient(self.Client):
            def _write_text(self, target, text, timeout_s):
                self.writes.append((target, text, timeout_s))
                return client.CallResult(
                    outcome="failed",
                    acceptance="unknown",
                    stage="test_text",
                    value=None,
                    error={"kind": "timeout"},
                )

        subject = UnknownTextClient()
        receipt = subject.submit(TARGET, "x", enter=True, timeout_s=3)
        self.assertEqual(subject.enters, 0)
        self.assertEqual(receipt.acceptance, "unknown")


class ClassificationTests(unittest.TestCase):
    def test_write_success_requires_exact_utf8_byte_count(self):
        text = "中文 😀 e\u0301"
        self.assertEqual(client.classify_write_success(text, {"bytes_written": len(text.encode())}), "queued")
        self.assertEqual(client.classify_write_success(text, {"bytes_written": 1}), "unknown")
        self.assertEqual(client.classify_write_success(text, {}), "unknown")

    def test_program_status_empty_is_not_idle(self):
        self.assertEqual(client.empty_program_status(), {"records": []})
        self.assertNotIn("idle", client.empty_program_status())

    def test_inspect_keeps_partial_results_and_errors(self):
        value = client.combine_inspect(
            process=completed({"pid": 7}),
            pwd=client.CallResult(
                outcome="failed",
                acceptance=None,
                stage="pwd",
                value=None,
                error={"kind": "native_error", "code": "not_found"},
            ),
            program_status=completed(client.empty_program_status()),
        )
        self.assertEqual(value["process"]["value"], {"pid": 7})
        self.assertEqual(value["pwd"]["error"]["code"], "not_found")
        self.assertEqual(value["program_status"]["value"], {"records": []})


class EventTests(unittest.TestCase):
    def test_event_stream_parser_rejects_truncated_lines(self):
        batch = client.parse_event_lines(
            TARGET,
            [
                json.dumps(status_envelope(SESSION_ID, BLOCK_ID, msg="valid")) + "\n",
                '{"type":"truncated"\n',
            ],
        )
        self.assertEqual(len(batch.events), 1)
        self.assertTrue(batch.gap_possible)
        self.assertEqual(batch.completion, "failed")

    def test_observe_requires_a_positive_finite_duration(self):
        with self.assertRaises(client.ValidationError):
            client.RexClient(ENDPOINT, runner=FakeRunner([])).observe(TARGET, 0)
        with self.assertRaises(client.ValidationError):
            client.RexClient(ENDPOINT, runner=FakeRunner([])).observe(TARGET, -1)

    def test_foreign_block_events_are_excluded_from_parser_and_observer(self):
        selected = status_envelope(SESSION_ID, BLOCK_ID, msg="selected")
        foreign = status_envelope(
            SESSION_ID, "block:018f5f6e-6d1a-7b31-8c20-fedcba987654", msg="foreign"
        )
        lines = [json.dumps(foreign) + "\n", json.dumps(selected) + "\n"]
        self.assertEqual(client.parse_event_lines(TARGET, lines).events, [selected])
        process = subprocess.Popen(
            [sys.executable, "-c", "import sys; sys.stdout.write(sys.argv[1])", "".join(lines)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        callbacks = []
        subject = client.RexClient(ENDPOINT)
        with patch.object(subject, "_start_events", return_value=process):
            batch = subject.observe(TARGET, 2, on_event=callbacks.append)
        self.assertEqual(batch.events, [selected])
        self.assertEqual(callbacks, [selected])


class ReviewRegressionTests(unittest.TestCase):
    def test_public_operations_reject_another_client_endpoint_before_io(self):
        foreign = replace(TARGET, endpoint=replace(ENDPOINT, server_address="other-host"))
        operations = [
            lambda c: c.resolve_target(foreign),
            lambda c: c.write_text(foreign, "x"),
            lambda c: c.press_enter(foreign),
            lambda c: c.submit(foreign, "x", True),
            lambda c: c.capture(foreign),
            lambda c: c.process(foreign),
            lambda c: c.pwd(foreign),
            lambda c: c.program_status(foreign),
            lambda c: c.inspect(foreign),
            lambda c: c.observe(foreign, 1),
        ]
        for operation in operations:
            runner = FakeRunner([])
            subject = client.RexClient(ENDPOINT, runner=runner)
            with self.subTest(operation=operation), self.assertRaises(client.ValidationError):
                operation(subject)
            self.assertEqual(runner.calls, [])

    def test_write_rejection_and_transport_unknown_remain_distinct(self):
        for native, expected in [
            (
                client.ProcessResult((), None, 3, 1, '{"code":"block_not_found"}', ""),
                "definitely_not_delivered",
            ),
            (
                client.ProcessResult((), None, 3, None, "", "", {"kind": "timeout"}),
                "unknown",
            ),
        ]:
            runner = FakeRunner([native])
            receipt = client.RexClient(ENDPOINT, runner=runner).submit(TARGET, "x", True)
            self.assertEqual(receipt.acceptance, expected)
            self.assertIsNone(receipt.enter_result)
            self.assertEqual(len(runner.calls), 1)


class ReceiverTests(unittest.TestCase):
    def test_frame_parser_preserves_payload_and_enter_is_separate(self):
        parser = receiver.FrameParser()
        payload = "中文 😀 e\u0301\n".encode()
        frame_bytes = (
            receiver.FRAME_PREFIX
            + b"unicode"
            + b"\t"
            + str(len(payload)).encode()
            + b"\n"
            + payload
            + receiver.FRAME_SUFFIX
        )
        frames = parser.feed(frame_bytes + b"\r")
        self.assertEqual(frames[0].payload, payload)
        self.assertEqual(frames[1], receiver.RawInput(b"\r"))

    def test_ack_is_explicit_receiver_read_evidence(self):
        payload = b"\x00\xff"
        decoded = receiver.ack_bytes("case", payload).decode()
        self.assertIn('"kind":"test_receiver_ack"', decoded)
        self.assertNotIn("consume", decoded)
        self.assertIn('"byte_count":2', decoded)

    def test_osc7501_uses_official_terminator(self):
        report = receiver.osc7501("working")
        self.assertTrue(report.startswith(b"\x1b]7501;"))
        self.assertTrue(report.endswith(b"\x1b\\"))
        self.assertIn(b"state=working", report)


class CliTests(unittest.TestCase):
    def test_query_timeout_reaches_public_function(self):
        with patch.object(client, "process", return_value=completed({"last_exit": None})) as query:
            with patch("sys.stdout", new=io.StringIO()):
                code = cli.main([
                    "--endpoint", ENDPOINT.server_address,
                    "--session", SESSION_ID, "--block", BLOCK_ID,
                    "--timeout", "180", "process",
                ])
        self.assertEqual(code, 0)
        query.assert_called_once_with(TARGET, 180.0)

    def test_submit_retains_its_command_timeout_option(self):
        args = cli._build_parser().parse_args([
            "--endpoint", ENDPOINT.server_address, "submit", "--timeout", "20"
        ])
        self.assertEqual(args.timeout, 20.0)

    def test_cli_submit_reads_stdin_without_stripping(self):
        stream = io.TextIOWrapper(io.BytesIO("  text\n".encode()))
        self.assertEqual(client.read_submit_text(stream), "  text\n")

    def test_no_shell_true_in_product_module(self):
        source = client.__file__
        with open(source, encoding="utf-8") as handle:
            self.assertNotIn("shell=True", handle.read())


def completed_with(value):
    return client.CallResult(
        outcome="completed", acceptance=None, stage="test", value=value, error=None
    )


def status_envelope(
    session_id: str,
    block_id: str,
    *,
    msg: str | None,
    state: str = "working",
    record_id: str = "ready",
) -> dict[str, object]:
    record: dict[str, object] = {"id": record_id, "state": state}
    if msg is not None:
        record["msg"] = msg
    return {
        "type": "session_event",
        "payload": {
            "session_id": session_id,
            "event": {
                "type": "block_event",
                "payload": {
                    "block_id": block_id,
                    "creator_name": client.TERMINAL_CREATOR,
                    "name": "program_status_changed",
                    "payload": {
                        "id": "ready",
                        "record": record,
                        "reason": "test",
                    },
                },
            },
        },
    }


class BinarySink:
    def __init__(self) -> None:
        self.buffer = io.BytesIO()


def osc7501_message(data: bytes) -> str:
    start = data.index(b"\x1b]7501;") + len(b"\x1b]7501;")
    end = data.index(b"\x1b\\", start)
    pairs: dict[str, str] = {}
    for part in data[start:end].decode("ascii").split(":"):
        if "=" in part:
            key, value = part.split("=", 1)
            pairs[key] = value
    return base64.b64decode(pairs["msg"]).decode("utf-8")


class TimeoutTests(unittest.TestCase):
    def test_nonfinite_deadlines_are_rejected(self):
        for value in (float("inf"), float("-inf"), float("nan")):
            with self.subTest(value=value):
                with self.assertRaises(client.ValidationError):
                    client.validate_timeout(value)

    def test_positive_finite_deadline_is_preserved(self):
        self.assertEqual(client.validate_timeout(180), 180.0)

class RemotePathTests(unittest.TestCase):
    def test_long_capture_rejects_a_gap_between_sampled_markers(self):
        captured = "\n".join(
            f"REXAPI-LONG-{index:08d}-payload" for index in (1, 2, 5)
        )
        with self.assertRaises(AssertionError):
            live_receiver.verify_long_output(captured, 5)
        complete = "\n".join(
            f"REXAPI-LONG-{index:08d}-payload" for index in range(1, 6)
        )
        live_receiver.verify_long_output(complete, 5)

    def test_remote_pwd_file_url_preserves_remote_path_semantics(self):
        self.assertTrue(
            live_receiver._pwd_matches(
                {"pwd": "file:///root/remote%20workspace"},
                "/root/remote workspace",
                resolve=False,
            )
        )
        self.assertFalse(
            live_receiver._pwd_matches(
                {"pwd": "file:///root/other"},
                "/root/remote workspace",
                resolve=False,
            )
        )


class EndpointTests(unittest.TestCase):
    def test_network_server_address_is_accepted(self):
        endpoint = replace(ENDPOINT, server_address="rex.example.ts.net")
        self.assertEqual(client.validate_endpoint(endpoint), endpoint)
        self.assertEqual(
            client.validate_endpoint(replace(ENDPOINT, server_address="tcp://10.0.0.9:9999")),
            replace(ENDPOINT, server_address="tcp://10.0.0.9:9999"),
        )

    def test_empty_server_address_is_rejected(self):
        for value in ("", "   "):
            with self.subTest(value=value), self.assertRaises(client.ValidationError):
                client.validate_endpoint(replace(ENDPOINT, server_address=value))

    def test_network_endpoint_reaches_build_argv(self):
        runner = FakeRunner(
            [
                client.ProcessResult(
                    argv=(),
                    input_text=None,
                    timeout_s=3,
                    returncode=0,
                    stdout="rex 0.1.0",
                    stderr="",
                ),
                client.ProcessResult(
                    argv=(),
                    input_text="{}",
                    timeout_s=3,
                    returncode=0,
                    stdout='{"api_revision":11}',
                    stderr="",
                ),
                client.ProcessResult(
                    argv=(),
                    input_text=None,
                    timeout_s=3,
                    returncode=0,
                    stdout='{"methods":[{"name":"session.create"}],"block_types":[]}',
                    stderr="",
                ),
                client.ProcessResult(
                    argv=(),
                    input_text="{}",
                    timeout_s=3,
                    returncode=0,
                    stdout='{"creators":[]}',
                    stderr="",
                ),
            ]
        )
        endpoint = replace(ENDPOINT, server_address="rex.example.ts.net")
        client.RexClient(endpoint, runner=runner).discover(timeout_s=3)
        self.assertEqual(runner.calls[0].argv[runner.calls[0].argv.index("--server") + 1], "rex.example.ts.net")


class ServerFactsTests(unittest.TestCase):
    def test_server_status_keeps_optional_host_id_absent(self):
        runner = FakeRunner(
            [client.ProcessResult(argv=(), input_text="{}", timeout_s=3, returncode=0,
                                  stdout='{"instance_id":"inst-1","rex_version":"0.1.0"}', stderr="")]
        )
        call = client.RexClient(ENDPOINT, runner=runner).server_status(timeout_s=3)
        self.assertEqual(call.outcome, "completed")
        self.assertEqual(call.value, {"instance_id": "inst-1", "rex_version": "0.1.0"})
        self.assertNotIn("host_id", call.value)

    def test_server_status_reports_host_id_when_present(self):
        runner = FakeRunner(
            [client.ProcessResult(argv=(), input_text="{}", timeout_s=3, returncode=0,
                                  stdout='{"host_id":"host-9","instance_id":"inst-2"}', stderr="")]
        )
        call = client.RexClient(ENDPOINT, runner=runner).server_status(timeout_s=3)
        self.assertEqual(call.value["host_id"], "host-9")
        self.assertEqual(call.value["instance_id"], "inst-2")

    def test_list_hosts_empty_is_a_valid_result(self):
        runner = FakeRunner(
            [client.ProcessResult(argv=(), input_text="{}", timeout_s=3, returncode=0,
                                  stdout='{"hosts":[]}', stderr="")]
        )
        call = client.RexClient(ENDPOINT, runner=runner).list_hosts(timeout_s=3)
        self.assertEqual(call.outcome, "completed")
        self.assertEqual(call.value, {"hosts": []})

    def test_server_status_uses_explicit_endpoint(self):
        runner = FakeRunner(
            [client.ProcessResult(argv=(), input_text="{}", timeout_s=3, returncode=0,
                                  stdout="{}", stderr="")]
        )
        endpoint = replace(ENDPOINT, server_address="rex.example.ts.net")
        client.RexClient(endpoint, runner=runner).server_status(timeout_s=3)
        call = runner.calls[0]
        self.assertEqual(call.argv[call.argv.index("--server") + 1], "rex.example.ts.net")
        self.assertIn("--autostart=false", call.argv)


class RemoteLaunchTests(unittest.TestCase):
    def test_remote_requires_argv_and_cwd_before_session_create(self):
        with self.assertRaises(client.ValidationError):
            live_receiver.receiver_launch(
                remote=True, receiver_argv_json=None, receiver_cwd="/remote/cwd", run_id="r1"
            )
        with self.assertRaises(client.ValidationError):
            live_receiver.receiver_launch(
                remote=True, receiver_argv_json='["python3","-u","/remote/receiver.py"]',
                receiver_cwd=None, run_id="r1",
            )

    def test_remote_argv_and_cwd_pass_through_without_local_resolution(self):
        command, cwd = live_receiver.receiver_launch(
            remote=True,
            receiver_argv_json='["/remote/bin/python3","-u","/remote/receiver.py","--run-id","{run_id}"]',
            receiver_cwd="/remote/some cwd",
            run_id="rid-42",
        )
        self.assertEqual(
            command,
            ["/remote/bin/python3", "-u", "/remote/receiver.py", "--run-id", "rid-42"],
        )
        self.assertEqual(cwd, "/remote/some cwd")
        self.assertNotIn(str(Path.cwd()), cwd)
        self.assertNotIn(live_receiver.sys.executable, command)

    def test_remote_rejects_non_argv_json_and_bad_items(self):
        for value in ('{"a":1}', '[]', '["python3", 5]', 'not-json'):
            with self.subTest(value=value), self.assertRaises(client.ValidationError):
                live_receiver.receiver_launch(
                    remote=True, receiver_argv_json=value, receiver_cwd="/remote", run_id="r1"
                )

    def test_local_launch_uses_local_interpreter_and_script(self):
        command, cwd = live_receiver.receiver_launch(
            remote=False, receiver_argv_json=None, receiver_cwd=None,
            run_id="r1", local_cwd="/tmp",
        )
        self.assertEqual(command[0], live_receiver.sys.executable)
        self.assertTrue(command[2].endswith("receiver.py"))
        self.assertEqual(cwd, str(Path("/tmp").resolve()))


class IsolationTests(unittest.TestCase):
    def test_failed_write_does_not_retry_or_switch_endpoint(self):
        runner = FakeRunner(
            [
                client.ProcessResult(argv=(), input_text="{}", timeout_s=3, returncode=1,
                                     stdout='{"code":"timeout"}', stderr="boom"),
            ]
        )
        client.RexClient(ENDPOINT, runner=runner).write_text(TARGET, "x", timeout_s=3)
        self.assertEqual(len(runner.calls), 1)
        call = runner.calls[0]
        self.assertEqual(call.argv[call.argv.index("--server") + 1], ENDPOINT.server_address)


class ReadinessReplayTests(unittest.TestCase):
    def test_receiver_reports_attempt_token_and_base_token_does_not_match(self):
        markers = live_receiver.ReadinessMarkers("ready-diagnostic")
        attempt = markers.next_token(0)
        self.assertEqual(attempt, "ready-diagnostic-0")
        markers.register(attempt)
        sink = BinarySink()
        receiver.Receiver(output=sink, run_id="diag").handle(
            receiver.Frame(case_id=f"ready:{attempt}", payload=b"ready-diagnostic")
        )
        reported = osc7501_message(sink.buffer.getvalue())
        self.assertEqual(reported, attempt)
        self.assertFalse(markers.observe("ready-diagnostic"))
        self.assertTrue(markers.observe(reported))
        self.assertEqual(markers.matched, attempt)

    def test_only_registered_attempt_token_within_envelope_matches(self):
        markers = live_receiver.ReadinessMarkers("ready-env")
        attempt = markers.next_token(3)
        markers.register(attempt)
        named = client.event_name_and_payload(
            status_envelope(SESSION_ID, BLOCK_ID, msg=attempt), TARGET
        )
        self.assertIsNotNone(named)
        name, payload = named
        self.assertEqual(name, "program_status_changed")
        self.assertTrue(markers.observe((payload.get("record") or {}).get("msg")))
        unsent = client.event_name_and_payload(
            status_envelope(SESSION_ID, BLOCK_ID, msg="ready-env-2"), TARGET
        )
        self.assertFalse(markers.observe((unsent[1].get("record") or {}).get("msg")))
        self.assertFalse(markers.observe(None))


class ReplayRex:
    def __init__(self, *, confirm_readiness: bool) -> None:
        self.confirm_readiness = confirm_readiness
        self._pending: list[dict[str, object]] = []
        self._state: str | None = None
        self.submits: list[str] = []

    def submit(self, target, text, enter, timeout_s):
        frames = receiver.FrameParser().feed(text.encode("utf-8"))
        frame = next(item for item in frames if isinstance(item, receiver.Frame))
        self.submits.append(frame.case_id)
        if frame.case_id == "status-working":
            self._state = "working"
            self._pending.append(
                status_envelope(
                    target.session_id, target.block_id, msg="working",
                    state="working", record_id="test",
                )
            )
        elif frame.case_id == "status-blocked":
            self._state = "blocked"
            self._pending.append(
                status_envelope(
                    target.session_id, target.block_id, msg="blocked",
                    state="blocked", record_id="test",
                )
            )
        elif frame.case_id.startswith("ready:") and self.confirm_readiness:
            attempt = frame.case_id.split(":", 1)[1]
            self._pending.append(
                status_envelope(target.session_id, target.block_id, msg=attempt)
            )
        return client.SubmitReceipt(
            target=target,
            text_result=completed_with({"bytes_written": len(text.encode("utf-8"))}),
            enter_result=None,
            acceptance="queued",
        )

    def observe(self, target, duration, cancel, *, on_event=None):
        events: list[dict[str, object]] = []
        while not cancel.is_set():
            while self._pending:
                event = self._pending.pop(0)
                events.append(event)
                if on_event is not None:
                    on_event(event)
            if self.confirm_readiness and self._state == "blocked" and len(events) >= 3:
                break
            time.sleep(0.005)
        return client.EventBatch(
            target=target, events=list(events), completion="cancelled",
            gap_possible=False, error=None,
        )

    def program_status(self, target, timeout_s):
        records = [] if self._state is None else [{"id": "test", "state": self._state}]
        return completed_with({"records": records})


OWNED = client.OwnedSession(
    endpoint=ENDPOINT, session_id=SESSION_ID, block_id=BLOCK_ID, creation_result={}
)


class ReadinessHarnessTests(unittest.TestCase):
    def test_successful_readiness_saves_batch_and_finds_attempt_token(self):
        with tempfile.TemporaryDirectory() as tmp:
            evidence = live_receiver.Evidence(Path(tmp))
            matched = live_receiver.observe_status_changes(
                ReplayRex(confirm_readiness=True),
                OWNED,
                evidence,
                attach_grace_s=0.0,
                marker_window_s=3.0,
                marker_step_wait_s=0.05,
            )
            self.assertTrue(matched)
            result = (Path(tmp) / "result.jsonl").read_text(encoding="utf-8")
            self.assertIn('"kind": "event_batch"', result)
            self.assertIn('"event_count": 3', result)
            self.assertTrue((Path(tmp) / "events.jsonl").exists())

    def test_readiness_failure_still_saves_batch_and_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            evidence = live_receiver.Evidence(Path(tmp))
            with self.assertRaises(TimeoutError):
                live_receiver.observe_status_changes(
                    ReplayRex(confirm_readiness=False),
                    OWNED,
                    evidence,
                    attach_grace_s=0.0,
                    marker_window_s=0.2,
                    marker_step_wait_s=0.02,
                )
            records = [
                json.loads(line)
                for line in (Path(tmp) / "result.jsonl").read_text(encoding="utf-8").splitlines()
            ]
            kinds = [record["kind"] for record in records]
            self.assertIn("event_batch", kinds)
            self.assertTrue((Path(tmp) / "events.jsonl").exists())

    def test_readiness_thread_not_returned_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            evidence = live_receiver.Evidence(Path(tmp))
            rex = ReplayRex(confirm_readiness=False)
            rex.observe = lambda *a, **k: time.sleep(5)  # thread never returns
            with self.assertRaises(TimeoutError):
                live_receiver.observe_status_changes(
                    rex,
                    OWNED,
                    evidence,
                    attach_grace_s=0.0,
                    marker_window_s=0.1,
                    marker_step_wait_s=0.02,
                )
            records = [
                json.loads(line)
                for line in (Path(tmp) / "result.jsonl").read_text(encoding="utf-8").splitlines()
            ]
            kinds = [record["kind"] for record in records]
            self.assertIn("event_batch_missing", kinds)


if __name__ == "__main__":
    unittest.main()
