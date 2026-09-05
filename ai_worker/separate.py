from __future__ import annotations

import contextlib
import os
import shutil
import stat
import wave
from pathlib import Path
from typing import Callable

from ai_worker.model_package import validate_demucs_package
from ai_worker.protocol import WorkerError, WorkerRequest, progress


SUPPORTED_MODELS = {"htdemucs", "htdemucs_ft"}


def _wav_duration(path: Path) -> float:
    try:
        info = path.lstat()
        if path.is_symlink() or not stat.S_ISREG(info.st_mode) or info.st_size <= 44:
            raise ValueError("not a regular wave")
        with wave.open(str(path), "rb") as audio:
            rate = audio.getframerate()
            frames = audio.getnframes()
        if rate <= 0 or frames <= 0:
            raise ValueError("empty wave")
        return frames / rate
    except (OSError, EOFError, wave.Error, ValueError) as error:
        raise WorkerError("AI_SEPARATION_OUTPUT_INVALID", "音源分离输出无效") from error


def _select_device(requested: str) -> str:
    if requested in {"cpu", "mps"}:
        return requested
    if requested != "auto":
        raise WorkerError("AI_REQUEST_INVALID", "AI 设备设置无效")
    try:
        import torch

        if torch.backends.mps.is_available():
            value = torch.ones(1, device="mps")
            if float(value.cpu()[0]) == 1.0:
                return "mps"
    except Exception:
        pass
    return "cpu"


def _demucs_separator(
    source: Path, output: Path, model: str, device: str, model_root: Path | None = None
) -> tuple[Path, Path]:
    from demucs.separate import main as demucs_main

    arguments = [
        "--two-stems",
        "vocals",
        "-n",
        model,
        "-d",
        device,
        "-o",
        str(output),
        str(source),
    ]
    if model_root is not None:
        arguments[arguments.index("-o"):arguments.index("-o")] = [
            "--repo",
            str(model_root),
        ]
    # A PyInstaller worker cannot invoke `sys.executable -m demucs.separate`:
    # that would re-enter this worker's own CLI. Demucs is collected into the
    # frozen runtime, so call its entrypoint in-process and isolate its prose and
    # progress bars from the JSON-lines protocol on stdout.
    with open(os.devnull, "w", encoding="utf-8") as sink:
        with contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
            demucs_main(arguments)
    track = source.stem
    nested = output / model / track
    vocals_source = nested / "vocals.wav"
    background_source = nested / "no_vocals.wav"
    vocals = output / "vocals.wav"
    background = output / "background_music.wav"
    if vocals_source != vocals:
        shutil.copy2(vocals_source, vocals)
    if background_source != background:
        shutil.copy2(background_source, background)
    return vocals, background


def separate_audio(
    request: WorkerRequest,
    separator: Callable[..., tuple[Path, Path]] = _demucs_separator,
    emit: Callable[[dict], None] = lambda _event: None,
) -> dict[str, str]:
    options = request.options
    if set(options) - {"model", "device", "modelRoot"}:
        raise WorkerError("AI_REQUEST_INVALID", "AI 请求选项无效")
    model = options.get("model", "htdemucs")
    if model not in SUPPORTED_MODELS:
        raise WorkerError("AI_MODEL_UNSUPPORTED", "不支持所选 Demucs 模型")
    device = _select_device(options.get("device", "auto"))
    model_root_value = options.get("modelRoot")
    model_root = None
    if model_root_value is not None:
        try:
            model_root = Path(model_root_value).resolve(strict=True)
        except (TypeError, OSError) as error:
            raise WorkerError("AI_REQUEST_INVALID", "Demucs 模型目录无效") from error
        if not model_root.is_dir():
            raise WorkerError("AI_REQUEST_INVALID", "Demucs 模型目录无效")
        validate_demucs_package(model_root, model)
    source_duration = _wav_duration(request.input_path)
    emit(progress("preparing", 10))
    try:
        try:
            vocals, background = separator(
                request.input_path, request.output_dir, model, device, model_root
            )
        except TypeError:
            # Backward-compatible injected fakes in focused unit tests take four arguments.
            vocals, background = separator(request.input_path, request.output_dir, model, device)
    except WorkerError:
        raise
    except BaseException as error:
        raise WorkerError("AI_SEPARATION_FAILED", "音源分离执行失败") from error
    emit(progress("validating", 80))
    resolved_output = request.output_dir.resolve(strict=True)
    resolved_paths = []
    for path in (Path(vocals), Path(background)):
        try:
            resolved = path.resolve(strict=True)
        except OSError as error:
            raise WorkerError("AI_SEPARATION_OUTPUT_INVALID", "音源分离输出无效") from error
        if not resolved.is_relative_to(resolved_output):
            raise WorkerError("AI_SEPARATION_OUTPUT_INVALID", "音源分离输出无效")
        resolved_paths.append(resolved)
    durations = [_wav_duration(path) for path in resolved_paths]
    tolerance = max(0.25, source_duration * 0.02)
    if any(abs(duration - source_duration) > tolerance for duration in durations):
        raise WorkerError("AI_SEPARATION_DURATION_MISMATCH", "分离音轨时长与原音频不一致")
    emit(progress("completed", 100))
    return {
        "vocalsPath": str(resolved_paths[0]),
        "backgroundMusicPath": str(resolved_paths[1]),
    }
