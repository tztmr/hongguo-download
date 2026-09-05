import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

from ai_worker.main import entrypoint, run_self_test
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
        worker_main = Mock(side_effect=lambda: calls.append("main") or 0)

        self.assertEqual(entrypoint(freeze_support=freeze_support, worker_main=worker_main), 0)
        self.assertEqual(calls, ["freeze", "main"])


if __name__ == "__main__":
    unittest.main()
