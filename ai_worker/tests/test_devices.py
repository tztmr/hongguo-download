import unittest
from types import SimpleNamespace

from ai_worker.protocol import WorkerError


class DeviceSelectionTests(unittest.TestCase):
    def test_auto_prefers_a_verified_cuda_device(self):
        from ai_worker.devices import select_device

        torch = fake_torch(cuda_available=True, architectures=["sm_120"])
        self.assertEqual(select_device("auto", torch_module=torch), "cuda:0")
        self.assertEqual(torch.cuda.tensor_runs, 1)

    def test_auto_uses_cpu_when_runtime_does_not_contain_gpu_architecture(self):
        from ai_worker.devices import select_device

        torch = fake_torch(cuda_available=True, capability=(12, 0), architectures=["sm_89"])
        self.assertEqual(select_device("auto", torch_module=torch), "cpu")

    def test_explicit_cuda_reports_an_actionable_error_when_unavailable(self):
        from ai_worker.devices import select_device

        with self.assertRaisesRegex(WorkerError, "AI_CUDA_UNAVAILABLE"):
            select_device("cuda", torch_module=fake_torch(cuda_available=False))

    def test_mps_behavior_is_preserved(self):
        from ai_worker.devices import select_device

        torch = fake_torch(mps_available=True)
        self.assertEqual(select_device("auto", torch_module=torch, platform="darwin"), "mps")


def fake_torch(
    *, cuda_available=False, mps_available=False, capability=(12, 0), architectures=None,
):
    architectures = architectures or []

    class Value:
        def __matmul__(self, _other):
            return self

        def mean(self):
            return self

        def cpu(self):
            return self

        def item(self):
            return 128.0

        def __getitem__(self, _index):
            return 1.0

        def __float__(self):
            return float(self.item())

    class Cuda:
        tensor_runs = 0

        @staticmethod
        def is_available():
            return cuda_available

        @staticmethod
        def get_device_capability(_index):
            return capability

        @staticmethod
        def get_arch_list():
            return architectures

        @staticmethod
        def synchronize(_index=None):
            return None

    def ones(*_args, **kwargs):
        if str(kwargs.get("device", "")).startswith("cuda"):
            Cuda.tensor_runs += 1
        return Value()

    return SimpleNamespace(
        cuda=Cuda,
        backends=SimpleNamespace(mps=SimpleNamespace(is_available=lambda: mps_available)),
        ones=ones,
    )


if __name__ == "__main__":
    unittest.main()
