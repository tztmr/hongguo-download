"""Streaming regressions; run with requirements-ai.txt installed."""
import importlib.util
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from ai_worker.protocol import WorkerError, WorkerRequest
from ai_worker.separate import _demucs_separator, _wav_duration, separate_audio


@unittest.skipUnless(importlib.util.find_spec("demucs"), "requires AI dependencies")
class StreamingSeparationTests(unittest.TestCase):
    @unittest.skipUnless(os.environ.get("HONGGUO_TEST_CUDA"), "requires CUDA GPU")
    @unittest.skipUnless(os.environ.get("HONGGUO_TEST_DEMUCS_MODEL"), "requires packaged Demucs weights")
    def test_real_cuda_oom_retry_reduces_forward_length_and_keeps_valid_audio(self):
        import numpy as np
        import soundfile as sf
        import torch
        from demucs.apply import apply_model
        from demucs.htdemucs import HTDemucs

        calls, lengths = [], []
        original_forward = HTDemucs.forward
        def forward(model, mix):
            lengths.append((mix.shape[-1], int(model.segment * model.samplerate)))
            return original_forward(model, mix)
        def infer(model, audio, **kwargs):
            calls.append(str(kwargs["device"]))
            if len(calls) == 1:
                raise RuntimeError("CUDA out of memory")
            return apply_model(model, audio, **kwargs)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "input.wav"
            sf.write(source, np.sin(np.arange(16000) * 0.08) * 0.2, 16000)
            with patch("demucs.apply.apply_model", side_effect=infer), patch.object(HTDemucs, "forward", forward):
                paths = _demucs_separator(source, root, "htdemucs", "cuda:0",
                    Path(os.environ["HONGGUO_TEST_DEMUCS_MODEL"]))
            self.assertTrue(all(device == "cuda:0" for device in calls))
            self.assertTrue(lengths)
            self.assertTrue(all(actual <= 4 * 44100 and padded <= 4 * 44100 for actual, padded in lengths))
            for path in paths:
                samples, rate = sf.read(path)
                self.assertEqual((len(samples), rate), (44100, 44100))
                self.assertTrue(np.isfinite(samples).all())

    @unittest.skipUnless(os.environ.get("HONGGUO_TEST_DEMUCS_MODEL"), "requires packaged Demucs weights")
    def test_real_model_with_restricted_torch_loading(self):
        import torch
        import soundfile as sf
        import numpy as np

        original_load = torch.load

        def restricted_load(*args, **kwargs):
            # Reproduce the Torch >= 2.6 default even with the macOS 2.5 runtime.
            self.assertNotEqual(kwargs.get("weights_only"), False)
            kwargs["weights_only"] = True
            return original_load(*args, **kwargs)

        with tempfile.TemporaryDirectory(prefix="中文 分离测试 ") as directory:
            root = Path(directory)
            source = root / "输入.wav"
            sf.write(source, np.sin(np.arange(16000) * 0.08) * 0.2, 16000)
            with patch("torch.load", side_effect=restricted_load):
                outputs = _demucs_separator(source, root, "htdemucs", "cpu",
                                            Path(os.environ["HONGGUO_TEST_DEMUCS_MODEL"]))
            for output in outputs:
                self.assertAlmostEqual(sf.info(output).duration, 1.0, places=2)

    def setUp(self):
        import numpy as np
        import soundfile as sf
        import torch

        self.np, self.sf, self.torch = np, sf, torch
        self.model = SimpleNamespace(
            samplerate=8000, audio_channels=2,
            sources=["drums", "bass", "other", "vocals"],
            cpu=lambda: None, eval=lambda: None,
        )

    def test_rf64_input_and_output_keep_their_real_duration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "长音轨.wav"
            self.sf.write(source, self.np.zeros(8000), 8000, format="RF64", subtype="PCM_16")
            self.assertEqual(source.read_bytes()[:4], b"RF64")
            self.assertEqual(_wav_duration(source), 1.0)
            request = WorkerRequest(1, "rf64", "separate", source, root, {"device": "cpu"})
            def stems(_source, output, *_args):
                paths = output / "vocals.wav", output / "background_music.wav"
                for path in paths:
                    self.sf.write(path, self.np.zeros(8000), 8000, format="RF64", subtype="PCM_16")
                return paths
            result = separate_audio(request, separator=stems)
            self.assertEqual(_wav_duration(Path(result["vocalsPath"])), 1.0)
            # Supporting RF64 must not disable the duration consistency check.
            self.sf.write(source, self.np.zeros(16000), 8000, format="RF64", subtype="PCM_16")
            with self.assertRaisesRegex(WorkerError, "AI_SEPARATION_DURATION_MISMATCH"):
                separate_audio(request, separator=stems)

    def test_output_crossing_wave_size_limit_switches_format_before_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "input.wav"
            self.sf.write(source, self.np.zeros(8000), 8000, subtype="PCM_16")
            with (
                # Exercise the real writer and validator without allocating 4 GiB.
                patch("ai_worker.separate.WAV_MAX_BYTES", 1024, create=True),
                patch("demucs.pretrained.get_model", return_value=self.model),
                patch("demucs.apply.apply_model", side_effect=AssertionError("silence needs no inference")),
            ):
                paths = _demucs_separator(source, root, "htdemucs", "cpu")
            for path in paths:
                with path.open("rb") as audio:
                    self.assertEqual(audio.read(4), b"RF64")
                self.assertEqual(_wav_duration(path), 1.0)
                self.assertEqual(self.sf.info(path).frames, 8000)

    def test_short_output_remains_standard_wave(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "input.wav"
            self.sf.write(source, self.np.zeros(8000), 8000, subtype="PCM_16")
            with patch("demucs.pretrained.get_model", return_value=self.model):
                paths = _demucs_separator(source, root, "htdemucs", "cpu")
            for path in paths:
                with path.open("rb") as audio:
                    self.assertEqual(audio.read(4), b"RIFF")
                self.assertEqual(_wav_duration(path), 1.0)

    def test_long_track_has_bounded_inference_and_sample_exact_seams(self):
        # Whole-season tensors, missing/duplicated overlap samples, or repeated
        # model loads must fail even though the expensive neural net is replaced.
        np, sf, torch = self.np, self.sf, self.torch
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "input.wav"
            block = (0.3 * np.sin(np.arange(8000) * 2 * np.pi * 100 / 8000)).astype("float32")
            with sf.SoundFile(source, "w", samplerate=8000, channels=1, subtype="PCM_16") as writer:
                for _ in range(1861):
                    writer.write(block)
                writer.write(block[:17])
            calls, events = [], []

            def infer(model, wav, **kwargs):
                self.assertLessEqual(wav.shape[-1], 124 * 8000)
                self.assertFalse(torch.is_grad_enabled())
                calls.append(wav.shape[-1])
                return torch.stack([torch.zeros_like(wav)] * 3 + [wav], dim=1)

            with (
                patch("demucs.pretrained.get_model", return_value=self.model) as loader,
                patch("demucs.apply.apply_model", side_effect=infer),
                patch("subprocess.run", side_effect=AssertionError("do not re-execute frozen worker")),
            ):
                vocals, background = _demucs_separator(source, root, "htdemucs", "cpu", root, emit=events.append)
            loader.assert_called_once_with("htdemucs", repo=root)
            self.assertGreater(len(calls), 1)
            for path in (vocals, background):
                self.assertEqual(sf.info(path).frames, 1861 * 8000 + 17)
            with sf.SoundFile(source) as original, sf.SoundFile(vocals) as result:
                while len(expected := original.read(8000, always_2d=True)):
                    actual = result.read(len(expected), always_2d=True)
                    np.testing.assert_allclose(actual, np.repeat(expected, 2, axis=1), atol=2 / 32768)
            percents = [event["percent"] for event in events]
            self.assertEqual(percents, sorted(percents))
            self.assertGreater(len(percents), 2)

    def test_silent_short_and_partial_final_chunks_stay_finite_and_keep_duration(self):
        np, sf = self.np, self.sf
        for frames in (1, 8000, 120 * 8000, 120 * 8000 + 1, 240 * 8000 + 17):
            with self.subTest(frames=frames), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "input.wav"
                sf.write(source, np.zeros(frames, dtype="float32"), 8000, subtype="PCM_16")
                with (
                    patch("demucs.pretrained.get_model", return_value=self.model),
                    patch("demucs.apply.apply_model", side_effect=AssertionError("silence needs no inference")),
                ):
                    paths = _demucs_separator(source, root, "htdemucs", "cpu")
                for path in paths:
                    actual, _ = sf.read(path)
                    self.assertEqual(len(actual), frames)
                    self.assertTrue(np.isfinite(actual).all())
                    self.assertEqual(np.count_nonzero(actual), 0)

    def test_failed_chunk_removes_partial_stems(self):
        np, sf, torch = self.np, self.sf, self.torch
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "input.wav"
            sf.write(source, np.tile([0.1, -0.1], 121 * 8000), 8000)
            calls = 0

            def infer(model, wav, **kwargs):
                nonlocal calls
                calls += 1
                if calls == 2:
                    raise RuntimeError("inference failed")
                return torch.stack([wav] * 4, dim=1)

            with (
                patch("demucs.pretrained.get_model", return_value=self.model),
                patch("demucs.apply.apply_model", side_effect=infer),
                self.assertRaises(RuntimeError),
            ):
                _demucs_separator(source, root, "htdemucs", "cpu")
            self.assertFalse((root / "vocals.wav").exists())
            self.assertFalse((root / "background_music.wav").exists())

    def test_resampling_keeps_final_frame_count_across_chunk_boundaries(self):
        np, sf, torch = self.np, self.sf, self.torch
        self.model.samplerate = 11025
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "input.wav"
            sf.write(source, np.tile([0.1, -0.1], 16001), 16000)
            with (
                patch("ai_worker.separate.CHUNK_SECONDS", 2),
                patch("demucs.pretrained.get_model", return_value=self.model),
                patch("demucs.apply.apply_model", side_effect=lambda model, wav, **kwargs: torch.stack([wav] * 4, dim=1)),
            ):
                paths = _demucs_separator(source, root, "htdemucs", "cpu")
            for path in paths:
                self.assertEqual(sf.info(path).samplerate, 11025)
                self.assertEqual(sf.info(path).frames, 22051)

    def test_mps_failure_retries_current_chunk_on_cpu_and_stays_on_cpu(self):
        np, sf, torch = self.np, self.sf, self.torch
        for failure in (
            NotImplementedError("Output channels > 65536 not supported at the MPS device"),
            RuntimeError("MPS backend out of memory"),
        ):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "input.wav"
                sf.write(source, np.tile([0.1, -0.1], 121 * 8000), 8000)
                devices, events = [], []

                def infer(model, wav, **kwargs):
                    devices.append(kwargs["device"])
                    if kwargs["device"] == "mps":
                        raise failure
                    return torch.stack([wav] * 4, dim=1)

                with (
                    patch("demucs.pretrained.get_model", return_value=self.model),
                    patch("demucs.apply.apply_model", side_effect=infer),
                    patch("torch.mps.empty_cache"),
                ):
                    paths = _demucs_separator(source, root, "htdemucs", "mps", emit=events.append)
                self.assertEqual(devices, ["mps", "cpu", "cpu", "cpu"])
                self.assertEqual(sf.info(paths[0]).frames, 242 * 8000)
                self.assertTrue(any("CPU" in event["stage"] for event in events))


if __name__ == "__main__":
    unittest.main()
