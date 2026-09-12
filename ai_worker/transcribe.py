from __future__ import annotations

import math
import os
import importlib
import sys
import wave
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Callable, Iterable, Mapping

from ai_worker.model_package import validate_whisper_package
from ai_worker.protocol import WorkerError, WorkerRequest, progress
from ai_worker.devices import select_device, accelerator_failure, release_accelerator_cache


SUPPORTED_MODELS = {"small", "medium"}


class TranscriptionProgress:
    """Adapt Whisper's decoded-frame cursor to our flushed JSON event stream."""

    def __init__(self, total: int, emit: Callable[[dict], None], **_kwargs):
        self.total = max(1, total)
        self.frames = 0
        self.emit = emit

    def __enter__(self):
        self.report()
        return self

    def update(self, frames: int):
        current = min(self.total, max(self.frames, self.frames + frames))
        if current != self.frames:
            self.frames = current
            self.report()

    def report(self):
        # Whisper mel frames are 10 ms (HOP_LENGTH / SAMPLE_RATE).
        def clock(frames):
            seconds = int(frames / 100)
            return f"{seconds // 3600:02d}:{seconds // 60 % 60:02d}:{seconds % 60:02d}"

        self.emit(progress(
            f"识别字幕 · 已处理 {clock(self.frames)} / {clock(self.total)}",
            10 + 75 * self.frames / self.total,
        ))

    def __exit__(self, *_args):
        # A failed decode must not fabricate completion.
        return False


def _format_timestamp(seconds: float) -> str:
    milliseconds = int(round(seconds * 1000))
    hours, milliseconds = divmod(milliseconds, 3_600_000)
    minutes, milliseconds = divmod(milliseconds, 60_000)
    whole_seconds, milliseconds = divmod(milliseconds, 1_000)
    return f"{hours:02d}:{minutes:02d}:{whole_seconds:02d},{milliseconds:03d}"


def segments_to_srt(segments: Iterable[Mapping[str, Any]], duration: float) -> str:
    if not math.isfinite(duration) or duration <= 0:
        raise WorkerError("SUBTITLE_INVALID", "字幕源时长无效")
    blocks = []
    previous_end = 0.0
    for segment in segments:
        text = str(segment.get("text", "")).strip()
        if not text:
            continue
        try:
            raw_start = float(segment["start"])
            raw_end = float(segment["end"])
        except (KeyError, TypeError, ValueError) as error:
            raise WorkerError("SUBTITLE_INVALID", "字幕时间轴无效") from error
        if not math.isfinite(raw_start) or not math.isfinite(raw_end):
            raise WorkerError("SUBTITLE_INVALID", "字幕时间轴无效")
        start = max(0.0, previous_end, raw_start)
        end = min(duration, raw_end)
        if start >= duration or end <= start:
            continue
        blocks.append(
            f"{len(blocks) + 1}\n{_format_timestamp(start)} --> {_format_timestamp(end)}\n{text}\n"
        )
        previous_end = end
    return "\n".join(blocks)


def _wav_duration(path: Path) -> float:
    try:
        with wave.open(str(path), "rb") as audio:
            rate = audio.getframerate()
            frames = audio.getnframes()
    except (OSError, EOFError, wave.Error) as error:
        raise WorkerError("AI_REQUEST_INVALID", "转写音频无效") from error
    if rate <= 0 or frames <= 0:
        raise WorkerError("AI_REQUEST_INVALID", "转写音频无效")
    return frames / rate


def _whisper_transcriber(
    source: Path,
    model: str,
    device: str,
    language: str | None,
    model_root: Path | None,
    emit: Callable[[dict], None],
) -> Mapping[str, Any]:
    emit(progress("loadingSubtitleModel", 2))
    import whisper

    protocol_stdout = sys.stdout

    def report(event):
        with redirect_stdout(protocol_stdout):
            emit(event)

    kwargs: dict[str, Any] = {"device": device}
    if model_root is not None:
        kwargs["download_root"] = str(model_root)
    # Each worker runs one request. Replace only transcribe's local tqdm binding,
    # leaving model downloads and other libraries' progress reporters untouched.
    module = importlib.import_module("whisper.transcribe")
    original = module.tqdm
    module.tqdm = SimpleNamespace(tqdm=lambda **kwargs: TranscriptionProgress(emit=report, **kwargs))
    try:
        with redirect_stdout(sys.stderr):
            loaded = whisper.load_model(model, **kwargs)
            report(progress("preparingSubtitleAudio", 5))
            return loaded.transcribe(
                _prepared_audio(source),
                language=language,
                fp16=device != "cpu",
                verbose=False,
            )
    finally:
        module.tqdm = original


def _prepared_audio(source: Path):
    """Reuse the desktop's 16 kHz mono PCM without another FFmpeg process."""
    import numpy as np
    try:
        with wave.open(str(source), "rb") as audio:
            if (audio.getframerate(), audio.getnchannels(), audio.getsampwidth(), audio.getcomptype()) != (16000, 1, 2, "NONE"):
                return str(source)
            samples = audio.readframes(audio.getnframes())
    except (OSError, EOFError, wave.Error):
        return str(source)
    return np.frombuffer(samples, dtype="<i2").astype(np.float32) / 32768.0


def transcribe_audio(
    request: WorkerRequest,
    transcriber: Callable[..., Mapping[str, Any]] = _whisper_transcriber,
    emit: Callable[[dict], None] = lambda _event: None,
) -> dict[str, Any]:
    options = request.options
    original_emit = emit
    highest_percent = 0.0

    def emit(event):
        nonlocal highest_percent
        if event.get("type") == "progress":
            highest_percent = max(highest_percent, event["percent"])
            event = {**event, "percent": highest_percent}
        original_emit(event)
    if set(options) - {"model", "device", "language", "modelRoot"}:
        raise WorkerError("AI_REQUEST_INVALID", "AI 请求选项无效")
    model = options.get("model", "small")
    if model not in SUPPORTED_MODELS:
        raise WorkerError("AI_MODEL_UNSUPPORTED", "不支持所选 Whisper 模型")
    emit(progress("preparingSubtitleAudio", 0))
    device = select_device(options.get("device", "auto"))
    language = options.get("language")
    if language is not None and (not isinstance(language, str) or len(language) > 16):
        raise WorkerError("AI_REQUEST_INVALID", "字幕语言设置无效")
    model_root_value = options.get("modelRoot")
    model_root = None
    if model_root_value is not None:
        try:
            model_root = Path(os.fspath(model_root_value)).resolve(strict=True)
        except (TypeError, OSError) as error:
            raise WorkerError("AI_REQUEST_INVALID", "Whisper 模型目录无效") from error
        if not model_root.is_dir():
            raise WorkerError("AI_REQUEST_INVALID", "Whisper 模型目录无效")
        validate_whisper_package(model_root, model)
    duration = _wav_duration(request.input_path)
    retry_cpu = False
    try:
        recognition = transcriber(request.input_path, model, device, language, model_root, emit)
    except WorkerError:
        raise
    except Exception as error:
        if options.get("device", "auto") == "auto" and accelerator_failure(error, device):
            retry_cpu = True
        else:
            raise WorkerError("AI_TRANSCRIPTION_FAILED", "语音转写执行失败") from error
    if retry_cpu:
        release_accelerator_cache(device)
        emit(progress("加速不可用，已切换 CPU 重新识别字幕", highest_percent))
        try:
            recognition = transcriber(request.input_path, model, "cpu", language, model_root, emit)
        except WorkerError:
            raise
        except Exception as error:
            raise WorkerError("AI_TRANSCRIPTION_FAILED", "语音转写执行失败") from error
    emit(progress("renderingSubtitles", 90))
    segments = recognition.get("segments") if isinstance(recognition, Mapping) else None
    if not isinstance(segments, list):
        raise WorkerError("AI_TRANSCRIPTION_FAILED", "语音转写结果无效")
    srt = segments_to_srt(segments, duration)
    if not srt:
        raise WorkerError("SUBTITLE_NO_SPEECH", "未识别到可用对白")
    output = request.output_dir.resolve(strict=True) / "subtitles.srt"
    temporary = output.with_suffix(".srt.tmp")
    with temporary.open("w", encoding="utf-8", newline="\n") as stream:
        stream.write(srt)
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(output)
    output.read_text(encoding="utf-8")
    emit(progress("completed", 100))
    detected_language = recognition.get("language")
    return {
        "srtPath": str(output),
        "language": detected_language if isinstance(detected_language, str) else "",
        "segmentCount": srt.count(" --> "),
    }
