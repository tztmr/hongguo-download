import unittest
from types import SimpleNamespace
from unittest.mock import patch

from ai_worker.separate import SeparationInference


class SeparationInferenceTests(unittest.TestCase):
    def runner(self, infer, device="cuda:0"):
        network = SimpleNamespace(cpu=lambda: None, max_allowed_segment=7.8,
                                  models=[SimpleNamespace(segment=7.8)])
        return SeparationInference(network, device, infer)

    def test_cuda_oom_retries_smaller_segment_and_keeps_gpu_for_next_chunk(self):
        calls = []
        def infer(_network, audio, **options):
            calls.append((options["device"], options.get("segment"), audio))
            if len(calls) == 1:
                raise RuntimeError("CUDA out of memory")
            self.assertEqual(_network.models[0].segment, 3.9)
            return "complete"
        runner = self.runner(infer)
        with patch("ai_worker.separate.release_accelerator_cache"):
            self.assertEqual(runner("chunk-1", lambda _: None), "complete")
            self.assertEqual(runner("chunk-2", lambda _: None), "complete")
        self.assertEqual(calls, [("cuda:0", None, "chunk-1"),
                                 ("cuda:0", 3.9, "chunk-1"),
                                 ("cuda:0", 3.9, "chunk-2")])

    def test_repeated_oom_falls_back_once_without_losing_current_chunk(self):
        calls = []
        def infer(_network, audio, **options):
            calls.append((options["device"], audio))
            if options["device"].startswith("cuda"):
                raise RuntimeError("CUDA out of memory")
            self.assertEqual(_network.models[0].segment, 7.8)
            return audio
        runner = self.runner(infer)
        with patch("ai_worker.separate.release_accelerator_cache"):
            self.assertEqual(runner("first", lambda _: None), "first")
            self.assertEqual(runner("second", lambda _: None), "second")
        self.assertEqual(calls, [("cuda:0", "first"), ("cuda:0", "first"),
                                 ("cpu", "first"), ("cpu", "second")])

    def test_input_errors_do_not_trigger_gpu_retry(self):
        calls = []
        def infer(*args, **kwargs):
            calls.append(1)
            raise RuntimeError("invalid audio shape")
        with self.assertRaisesRegex(RuntimeError, "invalid audio"):
            self.runner(infer)("chunk", lambda _: None)
        self.assertEqual(len(calls), 1)

    def test_cpu_failure_is_terminal(self):
        def infer(*args, **kwargs):
            raise RuntimeError("out of memory")
        with self.assertRaisesRegex(RuntimeError, "out of memory"):
            self.runner(infer, "cpu")("chunk", lambda _: None)
