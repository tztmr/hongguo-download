import tempfile
import unittest
import wave
from pathlib import Path

from ai_worker.protocol import WorkerError, WorkerRequest
from ai_worker.transcribe import segments_to_srt, transcribe_audio


def write_silence(path: Path, seconds: float = 2.0):
    rate = 8_000
    with wave.open(str(path), "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(rate)
        audio.writeframes(b"\0\0" * int(rate * seconds))


class TranscriptionTests(unittest.TestCase):
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

            def fake_transcriber(source, model, device, language, model_root):
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

            def packaged_transcriber(_source, model, _device, _language, received_root):
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
