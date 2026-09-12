import tempfile
import unittest
import wave
import io
import json
import sys
from contextlib import redirect_stdout, redirect_stderr
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

try:
    import numpy as np
except ImportError:
    np = None

from ai_worker.protocol import WorkerError, WorkerRequest, emit_event
from ai_worker.transcribe import segments_to_srt, transcribe_audio, _whisper_transcriber


def write_silence(path: Path, seconds: float = 2.0):
    rate = 8_000
    with wave.open(str(path), "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(rate)
        audio.writeframes(b"\0\0" * int(rate * seconds))


class TranscriptionTests(unittest.TestCase):
    def test_auto_cuda_failure_retries_once_and_publishes_cpu_subtitles(self):
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory), device="auto")
            devices, events = [], []

            def recognize(_source, _model, device, _language, _root, emit):
                devices.append(device)
                if device == "cuda:0":
                    emit({"type": "progress", "stage": "识别", "percent": 40})
                    raise RuntimeError("CUDA out of memory")
                emit({"type": "progress", "stage": "识别", "percent": 10})
                return {"segments": [{"start": 0, "end": 1, "text": "回退成功"}]}

            with patch("ai_worker.transcribe.select_device", return_value="cuda:0"):
                result = transcribe_audio(request, transcriber=recognize, emit=events.append)
            self.assertEqual(devices, ["cuda:0", "cpu"])
            self.assertIn("回退成功", Path(result["srtPath"]).read_text(encoding="utf-8"))
            self.assertEqual([e["percent"] for e in events], sorted(e["percent"] for e in events))
            self.assertTrue(any("CPU" in e["stage"] for e in events))

    def test_explicit_cuda_and_non_accelerator_errors_are_not_retried(self):
        for device, error in [("cuda", RuntimeError("CUDA out of memory")),
                              ("auto", OSError("disk full")),
                              ("auto", RuntimeError("unsupported audio format")),
                              ("auto", RuntimeError("invalid audio"))]:
            with self.subTest(device=device, error=error), tempfile.TemporaryDirectory() as directory:
                request = self.request(Path(directory), device=device)
                calls = []
                def fail(*args):
                    calls.append(args[2])
                    raise error
                with patch("ai_worker.transcribe.select_device", return_value="cuda:0"):
                    with self.assertRaisesRegex(WorkerError, "AI_TRANSCRIPTION_FAILED"):
                        transcribe_audio(request, transcriber=fail)
                self.assertEqual(calls, ["cuda:0"])
                self.assertFalse((request.output_dir / "subtitles.srt").exists())

    @unittest.skipIf(np is None, "requires numpy")
    def test_prepared_pcm_is_passed_directly_to_whisper(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "音频.wav"
            with wave.open(str(source), "wb") as audio:
                audio.setnchannels(1)
                audio.setsampwidth(2)
                audio.setframerate(16000)
                audio.writeframes(np.array([-32768, 0, 16384, 32767], dtype="<i2").tobytes())
            def recognize(audio, **kwargs):
                self.assertIsInstance(audio, np.ndarray)
                np.testing.assert_array_equal(audio, [-1, 0, 0.5, 32767 / 32768])
                self.assertFalse(kwargs["fp16"])
                return {"segments": []}
            whisper = SimpleNamespace(load_model=lambda *a, **kw: SimpleNamespace(transcribe=recognize))
            module = SimpleNamespace(tqdm=object())
            with patch.dict(sys.modules, {"whisper": whisper, "whisper.transcribe": module}):
                _whisper_transcriber(source, "small", "cpu", "zh", None, lambda _: None)

    def request(self, root: Path, **options):
        source = root / "dialogue.wav"
        write_silence(source)
        output = root / "outputs"
        output.mkdir()
        values = {"model": "small", "device": "cpu"}
        values.update(options)
        return WorkerRequest(1, "job-1", "transcribe", source, output, values)

    def test_segments_render_monotonic_utf8_srt(self):
        # Production mutation caught: invalid SRT numbering/timestamps or lossy Chinese output.
        text = segments_to_srt([{"start": 0.0, "end": 1.2, "text": "你好"}], duration=2.0)
        self.assertEqual(text, "1\n00:00:00,000 --> 00:00:01,200\n你好\n")
        text.encode("utf-8").decode("utf-8")

    def test_overlapping_segments_are_clamped_and_empty_segments_removed(self):
        # Production mutation caught: overlapping, negative, or out-of-bounds timestamps.
        text = segments_to_srt(
            [
                {"start": -1.0, "end": 1.0, "text": "第一句"},
                {"start": 0.5, "end": 3.0, "text": "第二句"},
                {"start": 1.5, "end": 1.7, "text": "   "},
            ],
            duration=2.0,
        )
        self.assertIn("00:00:00,000 --> 00:00:01,000", text)
        self.assertIn("00:00:01,000 --> 00:00:02,000", text)
        self.assertEqual(text.count(" --> "), 2)

    def test_transcription_writes_srt_and_reports_progress(self):
        # Production mutation caught: returning recognition data without an on-disk validated SRT.
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory))
            events = []

            def fake_transcriber(source, model, device, language, model_root, emit):
                self.assertEqual((source, model, device), (request.input_path, "small", "cpu"))
                self.assertIsNone(language)
                self.assertIsNone(model_root)
                return {"language": "zh", "segments": [{"start": 0.0, "end": 1.0, "text": "对白"}]}

            result = transcribe_audio(request, transcriber=fake_transcriber, emit=events.append)

            srt = Path(result["srtPath"])
            self.assertTrue(srt.resolve().is_relative_to(request.output_dir.resolve()))
            self.assertEqual(srt.read_text(encoding="utf-8"), "1\n00:00:00,000 --> 00:00:01,000\n对白\n")
            self.assertEqual(result["language"], "zh")
            self.assertEqual(result["segmentCount"], 1)
            percents = [event["percent"] for event in events]
            self.assertEqual(percents, sorted(percents))

    def test_no_speech_and_unsupported_model_are_rejected(self):
        # Production mutation caught: publishing blank subtitles or accepting arbitrary model code.
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory))
            with self.assertRaisesRegex(WorkerError, "SUBTITLE_NO_SPEECH"):
                transcribe_audio(
                    request,
                    transcriber=lambda *_: {"language": "zh", "segments": []},
                )
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory), model="large")
            with self.assertRaisesRegex(WorkerError, "AI_MODEL_UNSUPPORTED"):
                transcribe_audio(request, transcriber=lambda *_: {})

    def test_transcriber_failure_is_safe(self):
        # Production mutation caught: leaking model paths or command details in public errors.
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory))

            def failed(*_args):
                raise RuntimeError("/Users/private/model.pt token=secret")

            with self.assertRaisesRegex(WorkerError, "AI_TRANSCRIPTION_FAILED") as raised:
                transcribe_audio(request, transcriber=failed)
            self.assertNotIn("/Users/private", raised.exception.message)

    def test_whisper_cursor_streams_json_before_recognition_finishes(self):
        output, diagnostics = io.StringIO(), io.StringIO()
        original = object()
        module = SimpleNamespace(tqdm=original)

        def recognize(*_args, **kwargs):
            self.assertFalse(kwargs["verbose"])
            print("Detecting language")
            with module.tqdm.tqdm(total=9000, unit="frames", disable=False) as cursor:
                for expected in (35, 60, 85):
                    cursor.update(3000)
                    events = [json.loads(line) for line in output.getvalue().splitlines()]
                    self.assertEqual(events[-1]["percent"], expected)
            return {"language": "zh", "segments": []}

        whisper = SimpleNamespace(load_model=lambda *_args, **_kwargs: SimpleNamespace(transcribe=recognize))
        with patch.dict(sys.modules, {"whisper": whisper, "whisper.transcribe": module}), \
                redirect_stdout(output), redirect_stderr(diagnostics):
            _whisper_transcriber(Path("audio.wav"), "small", "cpu", "zh", None, emit_event)
        events = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual([event["percent"] for event in events], [2, 5, 10, 35, 60, 85])
        self.assertIn("00:01:30 / 00:01:30", events[-1]["stage"])
        self.assertIn("Detecting language", diagnostics.getvalue())
        self.assertIs(module.tqdm, original)

    def test_failed_decode_restores_reporter_without_faking_completion(self):
        events = []
        original = object()
        module = SimpleNamespace(tqdm=original)

        def fail(*_args, **_kwargs):
            with module.tqdm.tqdm(total=9000) as cursor:
                cursor.update(3000)
                cursor.update(-100)
                raise RuntimeError("decode failed")

        whisper = SimpleNamespace(load_model=lambda *_args, **_kwargs: SimpleNamespace(transcribe=fail))
        with patch.dict(sys.modules, {"whisper": whisper, "whisper.transcribe": module}):
            with self.assertRaises(RuntimeError):
                _whisper_transcriber(Path("audio.wav"), "small", "cpu", "zh", None, events.append)
        self.assertEqual(events[-1]["percent"], 35)
        self.assertIs(module.tqdm, original)

    def test_packaged_model_root_requires_the_selected_offline_checkpoint(self):
        # Production mutation caught: allowing Whisper to download a missing model at
        # task runtime instead of rejecting an incomplete signed component package.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            model_root = root / "model"
            model_root.mkdir()
            request = self.request(root, modelRoot=str(model_root))
            called = False

            def should_not_run(*_args):
                nonlocal called
                called = True
                return {}

            with self.assertRaisesRegex(WorkerError, "AI_MODEL_PACKAGE_INVALID"):
                transcribe_audio(request, transcriber=should_not_run)
            self.assertFalse(called)

            (model_root / "small.pt").write_bytes(b"weights")

            def packaged_transcriber(_source, model, _device, _language, received_root, _emit):
                self.assertEqual(model, "small")
                self.assertEqual(received_root, model_root.resolve())
                return {
                    "language": "zh",
                    "segments": [{"start": 0.0, "end": 1.0, "text": "离线对白"}],
                }

            result = transcribe_audio(request, transcriber=packaged_transcriber)
            self.assertEqual(result["segmentCount"], 1)


if __name__ == "__main__":
    unittest.main()
