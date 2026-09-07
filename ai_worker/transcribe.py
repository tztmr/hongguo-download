from __future__ import annotations

import math
import os
import wave
from pathlib import Path
from typing import Any, Callable, Iterable, Mapping

from ai_worker.model_package import validate_whisper_package
from ai_worker.protocol import WorkerError, WorkerRequest, progress
from ai_worker.devices import select_device


SUPPORTED_MODELS = {"small", "medium"}


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
) -> Mapping[str, Any]:
    import whisper

    kwargs: dict[str, Any] = {"device": device}
    if model_root is not None:
        kwargs["download_root"] = str(model_root)
    loaded = whisper.load_model(model, **kwargs)
    return loaded.transcribe(
        str(source),
        language=language,
        fp16=device != "cpu",
        verbose=False,
    )


def transcribe_audio(
    request: WorkerRequest,
    transcriber: Callable[[Path, str, str, str | None, Path | None], Mapping[str, Any]] = _whisper_transcriber,
    emit: Callable[[dict], None] = lambda _event: None,
) -> dict[str, Any]:
    options = request.options
    if set(options) - {"model", "device", "language", "modelRoot"}:
        raise WorkerError("AI_REQUEST_INVALID", "AI 请求选项无效")
    model = options.get("model", "small")
    if model not in SUPPORTED_MODELS:
        raise WorkerError("AI_MODEL_UNSUPPORTED", "不支持所选 Whisper 模型")
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
    emit(progress("transcribing", 10))
    try:
        recognition = transcriber(request.input_path, model, device, language, model_root)
    except WorkerError:
        raise
    except BaseException as error:
        raise WorkerError("AI_TRANSCRIPTION_FAILED", "语音转写执行失败") from error
    emit(progress("rendering", 85))
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
