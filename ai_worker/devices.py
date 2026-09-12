from __future__ import annotations

import sys
from typing import Any

from ai_worker.protocol import WorkerError


def select_device(
    requested: str,
    *,
    torch_module: Any | None = None,
    platform: str | None = None,
) -> str:
    """Select an accelerator only after a real operation succeeds."""
    if requested not in {"auto", "cpu", "cuda", "mps"}:
        raise WorkerError("AI_REQUEST_INVALID", "AI 设备设置无效")
    if requested == "cpu":
        return "cpu"

    torch = torch_module
    if torch is None:
        try:
            import torch as imported_torch

            torch = imported_torch
        except Exception as error:
            if requested == "auto":
                return "cpu"
            raise WorkerError("AI_ACCELERATOR_UNAVAILABLE", "所选 AI 加速设备不可用") from error

    if requested in {"auto", "cuda"}:
        try:
            if torch.cuda.is_available() and _cuda_runtime_supports_device(torch, 0):
                value = torch.ones((128, 128), device="cuda:0")
                result = (value @ value).mean().cpu()
                numeric = float(result.item() if hasattr(result, "item") else result)
                torch.cuda.synchronize(0)
                if numeric == 128.0:
                    return "cuda:0"
        except Exception as error:
            if requested == "cuda":
                raise WorkerError("AI_CUDA_UNAVAILABLE", "NVIDIA CUDA 加速不可用") from error
        if requested == "cuda":
            raise WorkerError("AI_CUDA_UNAVAILABLE", "NVIDIA CUDA 加速不可用")

    current_platform = platform or sys.platform
    if requested in {"auto", "mps"} and current_platform == "darwin":
        try:
            if torch.backends.mps.is_available():
                value = torch.ones(1, device="mps")
                if float(value.cpu()[0]) == 1.0:
                    return "mps"
        except Exception as error:
            if requested == "mps":
                raise WorkerError("AI_MPS_UNAVAILABLE", "Apple GPU 加速不可用") from error
        if requested == "mps":
            raise WorkerError("AI_MPS_UNAVAILABLE", "Apple GPU 加速不可用")

    return "cpu"


def _cuda_runtime_supports_device(torch: Any, index: int) -> bool:
    major, minor = torch.cuda.get_device_capability(index)
    target = major * 10 + minor
    architectures = torch.cuda.get_arch_list()
    if not architectures:
        return False
    for architecture in architectures:
        normalized = str(architecture).lower()
        ptx = normalized.startswith("compute_") or normalized.endswith("+ptx")
        code = normalized.removesuffix("+ptx").removeprefix("compute_").removeprefix("sm_")
        # Architecture-specific suffixes (90a, 100f) have narrower guarantees.
        if not code.isdigit() or not normalized.startswith(("sm_", "compute_")):
            continue
        compiled = int(code)
        if (ptx and compiled <= target) or (
            compiled // 10 == major and compiled <= target
        ):
            return True
    return False


def accelerator_failure(error: BaseException, device: str) -> bool:
    if not isinstance(error, (RuntimeError, NotImplementedError)):
        return False
    accelerator = "cuda" if device.startswith("cuda") else "mps" if device == "mps" else None
    if accelerator is None:
        return False
    message = str(error).lower()
    return accelerator in message or any(token in message for token in (
        "cublas", "cudnn", "out of memory", "no kernel image",
    ))


def release_accelerator_cache(device: str) -> None:
    # Call outside exception handlers so failed inference tracebacks are released.
    import gc
    gc.collect()
    try:
        import torch
        if device.startswith("cuda"):
            torch.cuda.empty_cache()
        elif device == "mps":
            torch.mps.empty_cache()
    except (ImportError, RuntimeError, AttributeError):
        pass  # A broken accelerator must not prevent CPU recovery.
