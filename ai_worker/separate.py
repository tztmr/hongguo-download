from __future__ import annotations

import contextlib
import inspect
import os
import stat
import wave
from fractions import Fraction
from pathlib import Path
from typing import Callable

from ai_worker.devices import select_device
from ai_worker.model_package import validate_demucs_package
from ai_worker.protocol import WorkerError, WorkerRequest, progress


SUPPORTED_MODELS = {"htdemucs", "htdemucs_ft"}
CHUNK_SECONDS = 120
CONTEXT_SECONDS = 1


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
    return select_device(requested)


def _demucs_separator(
    source: Path, output: Path, model: str, device: str, model_root: Path | None = None,
    *, emit: Callable[[dict], None] = lambda _event: None,
) -> tuple[Path, Path]:
    import numpy as np
    import soundfile as sf
    import torch
    from demucs.apply import apply_model
    from demucs.audio import convert_audio
    from demucs.pretrained import get_model
    from demucs.htdemucs import HTDemucs

    # Load once in-process: re-executing a frozen worker would re-enter its CLI.
    # Only library prose is redirected; our progress must reach the JSON pipe.
    with open(os.devnull, "w", encoding="utf-8") as sink:
        # Torch >= 2.6 defaults to restricted checkpoint loading. These are the
        # exact additional types in the shipped htdemucs / htdemucs_ft weights;
        # keep the restriction and scope the allowlist to this model load.
        model_types = [HTDemucs, Fraction, np.dtype, np.core.multiarray.scalar,
                       type(np.dtype(np.float64))]
        with (contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink),
              torch.serialization.safe_globals(model_types)):
            network = get_model(model, repo=model_root)
    network.cpu()
    network.eval()
    vocal_index = network.sources.index("vocals")
    rate = network.samplerate
    vocals = output / "vocals.wav"
    background = output / "background_music.wav"
    try:
        with sf.SoundFile(source) as reader, contextlib.ExitStack() as stack:
            total, input_rate = len(reader), reader.samplerate
            step = CHUNK_SECONDS * input_rate
            context = CONTEXT_SECONDS * input_rate
            count = (total + step - 1) // step
            # Standard PCM WAV remains compatible with wave.open validation.
            writers = [stack.enter_context(sf.SoundFile(
                path, "w", samplerate=rate, channels=network.audio_channels,
                subtype="PCM_16", format="WAV",
            )) for path in (vocals, background)]
            pending = None
            for index in range(count):
                start = max(0, index * step - context)
                end = min(total, (index + 1) * step + context)
                reader.seek(start)
                samples = reader.read(end - start, dtype="float32", always_2d=True)
                emit(progress(f"分离第 {index + 1}/{count} 段", 10 + 65 * index / count))
                with torch.inference_mode():
                    wav = convert_audio(torch.from_numpy(samples.T.copy()), input_rate,
                                        rate, network.audio_channels)
                    length = round(end * rate / input_rate) - round(start * rate / input_rate)
                    wav = wav[:, :length]
                    ref = wav.mean(0)
                    mean, std = ref.mean(), ref.std(unbiased=False)
                    if std < 1e-8:
                        stems = torch.stack((wav, torch.zeros_like(wav)))
                    else:
                        normalized = (wav - mean) / std

                        def infer():
                            return apply_model(
                                network, normalized[None], device=device, shifts=1,
                                split=True, overlap=0.25, progress=False, num_workers=0,
                            )[0]

                        retry_on_cpu = False
                        try:
                            sources = infer()
                        except (RuntimeError, NotImplementedError) as error:
                            message = str(error).lower()
                            accelerator = "mps" if device == "mps" else "cuda" if device.startswith("cuda") else None
                            if accelerator is None or accelerator not in message and not any(
                                token in message for token in (
                                    "out of memory", "not supported", "not implemented", "no kernel image",
                                    "not currently implemented", "unsupported",
                                )
                            ):
                                raise
                            retry_on_cpu = True
                        # Leave the exception handler first to release its traceback
                        # and failed GPU tensors before allocating the CPU retry.
                        if retry_on_cpu:
                            device = "cpu"
                            network.cpu()
                            if accelerator == "mps":
                                torch.mps.empty_cache()
                            else:
                                torch.cuda.empty_cache()
                            emit(progress("加速不可用，已切换 CPU 继续分离", 10 + 65 * index / count))
                            sources = infer()
                        sources = sources.cpu() * std + mean
                        voice = sources[vocal_index]
                        music = sources.sum(dim=0) - voice
                        stems = torch.stack((voice, music))
                        del sources, voice, music, normalized
                    current = stems.permute(0, 2, 1).contiguous().numpy()
                    if not np.isfinite(current).all():
                        raise WorkerError("AI_SEPARATION_OUTPUT_INVALID", "音源分离输出无效")
                    if pending is not None:
                        overlap = pending.shape[1]
                        fade = np.linspace(0, 1, overlap, dtype="float32")[None, :, None]
                        current[:, :overlap] = pending * (1 - fade) + current[:, :overlap] * fade
                    # Hold only the boundary shared with the next chunk. Copy it
                    # so a tiny tail does not retain the entire tensor allocation.
                    keep = length if index + 1 == count else (
                        round(((index + 1) * step - context) * rate / input_rate)
                        - round(start * rate / input_rate)
                    )
                    pending = current[:, keep:].copy() if index + 1 < count else None
                    for writer, stem in zip(writers, current):
                        writer.write(stem[:keep])
                    del stem, stems, current, wav, ref, samples
                emit(progress(f"已分离 {index + 1}/{count} 段", 10 + 65 * (index + 1) / count))
    except BaseException:
        for path in (vocals, background):
            path.unlink(missing_ok=True)
        raise
    return vocals, background


def separate_audio(
    request: WorkerRequest,
    separator: Callable[..., tuple[Path, Path]] | None = None,
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
        if separator is None:
            vocals, background = _demucs_separator(
                request.input_path, request.output_dir, model, device, model_root, emit=emit,
            )
        else:
            arguments = (request.input_path, request.output_dir, model, device, model_root)
            try:
                inspect.signature(separator).bind(*arguments)
            except TypeError:
                arguments = arguments[:4]
            vocals, background = separator(*arguments)
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
