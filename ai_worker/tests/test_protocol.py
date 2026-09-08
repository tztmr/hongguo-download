import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from ai_worker.main import configure_utf8_stdio, entrypoint, run_self_test
from ai_worker.protocol import WorkerError, emit_event, parse_request, safe_error


class ProtocolTests(unittest.TestCase):
    def request(self, root: Path, **overrides):
        input_path = root / "input.wav"
        input_path.write_bytes(b"RIFF-fixture")
        output_dir = root / "outputs"
        output_dir.mkdir(exist_ok=True)
        value = {
            "version": 1,
            "jobId": "job-1",
            "operation": "separate",
            "inputPath": str(input_path),
            "outputDir": str(output_dir),
            "options": {"model": "htdemucs"},
        }
        value.update(overrides)
        return value

    def test_request_rejects_unknown_operation_and_outside_output(self):
        # Production mutation caught: accepting arbitrary operations or output escape paths.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(WorkerError, "AI_REQUEST_INVALID"):
                parse_request(self.request(root, operation="shell"))
            outside = root.parent / f"{root.name}-outside"
            outside.mkdir()
            self.addCleanup(lambda: outside.rmdir())
            with self.assertRaisesRegex(WorkerError, "AI_REQUEST_INVALID"):
                parse_request(self.request(root, outputDir=str(outside)))

    def test_request_rejects_unknown_fields_symlinks_and_missing_input(self):
        # Production mutation caught: schema drift or following an attacker-controlled input link.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(WorkerError, "AI_REQUEST_INVALID"):
                parse_request(self.request(root, surprise=True))
            target = root / "target.wav"
            target.write_bytes(b"audio")
            link = root / "linked.wav"
            link.symlink_to(target)
            with self.assertRaisesRegex(WorkerError, "AI_REQUEST_INVALID"):
                parse_request(self.request(root, inputPath=str(link)))
            with self.assertRaisesRegex(WorkerError, "AI_REQUEST_INVALID"):
                parse_request(self.request(root, inputPath=str(root / "missing.wav")))

    def test_errors_never_include_environment_command_line_or_paths(self):
        # Production mutation caught: reflecting exception text into the public JSON protocol.
        event = safe_error(RuntimeError("token=synthetic-secret /Users/name/file --password x"))
        self.assertEqual(event["code"], "AI_WORKER_FAILED")
        serialized = json.dumps(event)
        self.assertNotIn("synthetic-secret", serialized)
        self.assertNotIn("/Users/name", serialized)
        self.assertNotIn("password", serialized)

    def test_emit_event_outputs_one_compact_json_line(self):
        # Production mutation caught: logging prose or pretty JSON to stdout.
        stream = io.StringIO()
        with redirect_stdout(stream):
            emit_event({"type": "progress", "stage": "ready", "percent": 1})
        lines = stream.getvalue().splitlines()
        self.assertEqual(len(lines), 1)
        self.assertEqual(json.loads(lines[0]), {"type": "progress", "stage": "ready", "percent": 1})

    def test_emit_event_encodes_chinese_paths_as_ascii_json(self):
        # Production mutation caught: ensure_ascii=False emitting GBK/UTF-8
        # Chinese bytes that Windows pipes cannot read as UTF-8 lines.
        stream = io.StringIO()
        path = r"D:\红果下载\001_htdemucs_人声.wav"
        emit_event({"type": "result", "outputs": {"vocalsPath": path}}, stream=stream)
        line = stream.getvalue()
        self.assertTrue(line.isascii(), line)
        self.assertIn("\\u", line)
        self.assertEqual(json.loads(line)["outputs"]["vocalsPath"], path)

    def test_self_test_uses_injected_importer_without_network_and_emits_json(self):
        # Production mutation caught: self-test downloading weights or omitting protocol proof.
        imported = []
        stream = io.StringIO()

        def importer(name):
            imported.append(name)
            return {
                "torch": SimpleNamespace(Tensor=object),
                "demucs.pretrained": SimpleNamespace(get_model=lambda _: None),
                "demucs.apply": SimpleNamespace(apply_model=lambda *_: None),
                "whisper": SimpleNamespace(load_model=lambda *_: None),
            }[name]

        with redirect_stdout(stream):
            self.assertEqual(run_self_test(importer=importer), 0)
        self.assertEqual(imported, ["torch", "demucs.pretrained", "demucs.apply", "whisper"])
        events = [json.loads(line) for line in stream.getvalue().splitlines()]
        self.assertEqual([event["type"] for event in events], ["progress", "result"])

    def test_entrypoint_initializes_frozen_multiprocessing_before_main(self):
        calls = []
        freeze_support = Mock(side_effect=lambda: calls.append("freeze"))
        configure_stdio = Mock(side_effect=lambda: calls.append("stdio"))
        worker_main = Mock(side_effect=lambda: calls.append("main") or 0)

        self.assertEqual(
            entrypoint(
                freeze_support=freeze_support,
                worker_main=worker_main,
                configure_stdio=configure_stdio,
            ),
            0,
        )
        self.assertEqual(calls, ["freeze", "stdio", "main"])

    def test_configure_utf8_stdio_reconfigures_stdin_and_stdout(self):
        stdout = Mock()
        stdin = Mock()
        with patch("ai_worker.main.sys.stdout", stdout), patch(
            "ai_worker.main.sys.stdin", stdin
        ):
            configure_utf8_stdio()
        stdout.reconfigure.assert_called_with(encoding="utf-8", errors="replace")
        stdin.reconfigure.assert_called_with(encoding="utf-8", errors="replace")


if __name__ == "__main__":
    unittest.main()
