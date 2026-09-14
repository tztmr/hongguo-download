from __future__ import annotations

import contextlib
import inspect
import os
import stat
import wave
from fractions import Fraction
from pathlib import Path
from typing import Callable

from ai_worker.devices import select_device, accelerator_failure, release_accelerator_cache
from ai_worker.model_package import validate_demucs_package
from ai_worker.protocol import WorkerError, WorkerRequest, progress


SUPPORTED_MODELS = {"htdemucs", "htdemucs_ft"}
CHUNK_SECONDS = 120
CONTEXT_SECONDS = 1
WAV_MAX_BYTES = (1 << 32) - 1


class SeparationInference:
    """Keep successful retry settings for all remaining streaming chunks."""

    def __init__(self, network, device, apply_model):
        self.network, self.device, self.apply_model = network, device, apply_model
        self.segment = None
        self.original_segments = [(part, part.segment)
                                  for part in getattr(network, "models", [network])
                                  if hasattr(part, "segment")]

    def __call__(self, audio, report):
        while True:
            smaller_segment = False
            try:
                kwargs = {} if self.segment is None else {"segment": self.segment}
                return self.apply_model(
                    self.network, audio, device=self.device, shifts=1,
                    split=True, overlap=0.25, progress=False, num_workers=0, **kwargs,
                )
            except (RuntimeError, NotImplementedError) as error:
                if not accelerator_failure(error, self.device):
                    raise
                smaller_segment = (
                    self.device.startswith("cuda") and self.segment is None
                    and "out of memory" in str(error).lower()
                )
            # Do not retain the failed call's traceback/tensors during recovery.
            old_device = self.device
            if smaller_segment:
                allowed = float(getattr(self.network, "max_allowed_segment",
                                        getattr(self.network, "segment", 7.8)))
                self.segment = min(4.0, allowed / 2)
                # HTDemucs pads its forward input to model.segment even when
                # apply_model(segment=...) is shorter. Bound both values so an
                # OOM retry actually uses less GPU memory.
                for part, _original in self.original_segments:
                    part.segment = self.segment
                release_accelerator_cache(old_device)
                report("显存不足，缩短推理片段后继续使用 NVIDIA GPU")
            else:
                self.device, self.segment = "cpu", None
                for part, original in self.original_segments:
                    part.segment = original
                self.network.cpu()
                release_accelerator_cache(old_device)
                report("加速不可用，已切换 CPU 继续分离")


def _wav_duration(path: Path) -> float:
    try:
        info = path.lstat()
        if path.is_symlink() or not stat.S_ISREG(info.st_mode) or info.st_size <= 44:
            raise ValueError("not a regular wave")
        with path.open("rb") as header:
            rf64 = header.read(4) == b"RF64"
        if rf64:
            # FFmpeg already produces RF64 for very long preprocessed inputs.
            # Python's wave module only understands the 32-bit RIFF variant.
            import soundfile as sf
            with sf.SoundFile(str(path)) as audio:
                rate, frames = audio.samplerate, len(audio)
        else:
            with wave.open(str(path), "rb") as audio:
                rate = audio.getframerate()
                frames = audio.getnframes()
        if rate <= 0 or frames <= 0:
            raise ValueError("empty wave")
        return frames / rate
    except (OSError, EOFError, wave.Error, ValueError, RuntimeError) as error:
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
    inference = SeparationInference(network, device, apply_model)
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
            # 44.1 kHz stereo PCM reaches RIFF's 4 GiB limit after ~6.8 hours.
            # Choose the 64-bit header before writing; retries cannot repair an
            # overflowed 32-bit header. Keep ordinary WAV for shorter exports.
            output_frames = (total * rate + input_rate - 1) // input_rate
            output_format = "RF64" if (
                output_frames * network.audio_channels * 2 + 4096 > WAV_MAX_BYTES
            ) else "WAV"
            writers = [stack.enter_context(sf.SoundFile(
                path, "w", samplerate=rate, channels=network.audio_channels,
                subtype="PCM_16", format=output_format,
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

                        sources = inference(
                            normalized[None],
                            lambda stage: emit(progress(stage, 10 + 65 * index / count)),
                        )[0]
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
        raise WorkerError("AI_SEPARATION_DURATION_MISMATCH",
                          f"分离音轨时长与原音频不一致（源 {source_duration:.2f} 秒，"
                          f"人声 {durations[0]:.2f} 秒，背景音乐 {durations[1]:.2f} 秒）")
    emit(progress("completed", 100))
    return {
        "vocalsPath": str(resolved_paths[0]),
        "backgroundMusicPath": str(resolved_paths[1]),
    }
