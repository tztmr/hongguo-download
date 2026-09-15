import tempfile
import unittest
import wave
from pathlib import Path
from unittest.mock import patch

from ai_worker.protocol import WorkerError, WorkerRequest
from ai_worker.separate import separate_audio


def write_silence(path: Path, seconds: float = 1.0):
    rate = 8_000
    frames = int(rate * seconds)
    with wave.open(str(path), "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(rate)
        audio.writeframes(b"\0\0" * frames)


class SeparationTests(unittest.TestCase):
    def test_windows_auto_without_cuda_runs_the_cpu_separator_and_reports_cpu(self):
        from ai_worker.devices import select_device
        from ai_worker.tests.test_devices import fake_torch
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory))
            request.options['device'] = 'auto'
            events, used_devices = [], []
            def backend(_source, output, _model, device):
                used_devices.append(device)
                paths = output / 'vocals.wav', output / 'background_music.wav'
                for path in paths:
                    write_silence(path)
                return paths
            def choose(requested):
                return select_device(requested, torch_module=fake_torch(cuda_available=False), platform='win32')
            with patch('ai_worker.separate._select_device', side_effect=choose):
                result = separate_audio(request, separator=backend, emit=events.append)
            self.assertEqual(used_devices, ['cpu'])
            self.assertTrue(Path(result['vocalsPath']).exists())
            self.assertIn('CPU', events[0]['stage'])

    def request(self, root: Path, model: str = "htdemucs"):
        source = root / "input.wav"
        write_silence(source)
        output = root / "outputs"
        output.mkdir()
        return WorkerRequest(1, "job-1", "separate", source, output, {"model": model, "device": "cpu"})

    def test_separation_returns_both_stems_and_monotonic_progress(self):
        # Production mutation caught: publishing one stem or regressing progress.
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory))
            events = []

            def fake_separator(source, output, model, device):
                self.assertEqual((model, device), ("htdemucs", "cpu"))
                vocals = output / "vocals.wav"
                background = output / "background_music.wav"
                write_silence(vocals)
                write_silence(background)
                return vocals, background

            result = separate_audio(request, separator=fake_separator, emit=events.append)

            self.assertTrue(Path(result["vocalsPath"]).is_file())
            self.assertTrue(Path(result["backgroundMusicPath"]).is_file())
            percents = [event["percent"] for event in events]
            self.assertEqual(percents, sorted(percents))
            self.assertEqual(percents[-1], 100)

    def test_default_separator_receives_live_progress_callback(self):
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory))
            events = []

            def backend(source, output, model, device, model_root, *, emit):
                emit({"type": "progress", "stage": "分离第 1/2 段", "percent": 40})
                paths = output / "vocals.wav", output / "background_music.wav"
                for path in paths:
                    write_silence(path)
                return paths

            with patch("ai_worker.separate._demucs_separator", side_effect=backend):
                separate_audio(request, emit=events.append)
            self.assertIn(40, [event["percent"] for event in events])

    def test_missing_or_zero_length_stem_is_rejected(self):
        # Production mutation caught: treating incomplete Demucs output as success.
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory))

            def missing(_source, output, _model, _device):
                vocals = output / "vocals.wav"
                write_silence(vocals)
                return vocals, output / "missing.wav"

            with self.assertRaisesRegex(WorkerError, "AI_SEPARATION_OUTPUT_INVALID"):
                separate_audio(request, separator=missing)

            def empty(_source, output, _model, _device):
                vocals = output / "vocals.wav"
                background = output / "background_music.wav"
                vocals.write_bytes(b"")
                write_silence(background)
                return vocals, background

            with self.assertRaisesRegex(WorkerError, "AI_SEPARATION_OUTPUT_INVALID"):
                separate_audio(request, separator=empty)

    def test_duration_mismatch_and_unsupported_model_are_rejected(self):
        # Production mutation caught: accepting desynchronized stems or arbitrary model names.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            request = self.request(root)

            def mismatched(_source, output, _model, _device):
                vocals = output / "vocals.wav"
                background = output / "background_music.wav"
                write_silence(vocals, 0.1)
                write_silence(background, 0.1)
                return vocals, background

            with self.assertRaisesRegex(WorkerError, "AI_SEPARATION_DURATION_MISMATCH"):
                separate_audio(request, separator=mismatched)

        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory), model="arbitrary-model")
            with self.assertRaisesRegex(WorkerError, "AI_MODEL_UNSUPPORTED"):
                separate_audio(request, separator=lambda *_: ())

    def test_separator_failure_is_mapped_to_safe_worker_error(self):
        # Production mutation caught: leaking tool exceptions through the protocol.
        with tempfile.TemporaryDirectory() as directory:
            request = self.request(Path(directory))

            def failed(*_args):
                raise RuntimeError("/Users/private model command failed")

            with self.assertRaisesRegex(WorkerError, "AI_SEPARATION_FAILED") as raised:
                separate_audio(request, separator=failed)
            self.assertNotIn("/Users/private", raised.exception.message)

    def test_packaged_model_root_requires_named_bag_and_all_weight_files(self):
        # Production mutation caught: accepting an incomplete offline model package and
        # letting Demucs fail later (or fall back to a network-backed repository).
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            model_root = root / "model"
            model_root.mkdir()
            request = self.request(root)
            request.options["modelRoot"] = str(model_root)
            called = False

            def should_not_run(*_args):
                nonlocal called
                called = True
                return ()

            with self.assertRaisesRegex(WorkerError, "AI_MODEL_PACKAGE_INVALID"):
                separate_audio(request, separator=should_not_run)
            self.assertFalse(called)

            (model_root / "htdemucs.yaml").write_text(
                "models: ['955717e8']\n", encoding="utf-8"
            )
            (model_root / "955717e8-deadbeef.th").write_bytes(b"weights")

            def packaged_separator(_source, output, model, _device, received_root):
                self.assertEqual(model, "htdemucs")
                self.assertEqual(received_root, model_root.resolve())
                vocals = output / "vocals.wav"
                background = output / "background_music.wav"
                write_silence(vocals)
                write_silence(background)
                return vocals, background

            result = separate_audio(request, separator=packaged_separator)
            self.assertTrue(Path(result["vocalsPath"]).is_file())


if __name__ == "__main__":
    unittest.main()
