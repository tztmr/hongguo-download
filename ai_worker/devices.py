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
    supported = set()
    has_ptx = False
    for architecture in architectures:
        normalized = str(architecture).lower().replace("compute_", "sm_")
        if normalized.endswith("+ptx"):
            has_ptx = True
            normalized = normalized[:-4]
        if normalized.startswith("sm_"):
            try:
                supported.add(int(normalized[3:]))
            except ValueError:
                continue
    return target in supported or (has_ptx and bool(supported) and target >= max(supported))
