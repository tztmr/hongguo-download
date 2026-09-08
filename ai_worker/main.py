from __future__ import annotations

import importlib
import json
import logging
import multiprocessing
import sys
import tempfile
from pathlib import Path
from typing import Callable

from ai_worker.protocol import WorkerError, emit_event, parse_request, progress, result, safe_error


logging.basicConfig(stream=sys.stderr, level=logging.WARNING)


def run_self_test(importer: Callable[[str], object] = importlib.import_module) -> int:
    torch = importer("torch")
    demucs_pretrained = importer("demucs.pretrained")
    demucs_apply = importer("demucs.apply")
    whisper = importer("whisper")
    required = [
        getattr(torch, "Tensor", None),
        getattr(demucs_pretrained, "get_model", None),
        getattr(demucs_apply, "apply_model", None),
        getattr(whisper, "load_model", None),
    ]
    if any(value is None for value in required):
        raise WorkerError("AI_RUNTIME_INVALID", "AI 运行环境自检失败")
    with tempfile.TemporaryDirectory(prefix="hongguo-ai-self-test-") as directory:
        marker = Path(directory) / "ready"
        marker.write_text("ok", encoding="utf-8")
        emit_event(progress("selfTesting", 50))
        emit_event(result({"selfTest": True}))
    return 0


def dispatch(request):
    if request.operation == "separate":
        from ai_worker.separate import separate_audio

        return separate_audio(request, emit=emit_event)
    if request.operation == "transcribe":
        from ai_worker.transcribe import transcribe_audio

        return transcribe_audio(request, emit=emit_event)
    raise WorkerError("AI_REQUEST_INVALID", "AI 请求无效")


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    try:
        if argv == ["--self-test"]:
            return run_self_test()
        if argv:
            raise WorkerError("AI_REQUEST_INVALID", "AI 请求无效")
        line = sys.stdin.readline()
        if not line or sys.stdin.readline():
            raise WorkerError("AI_REQUEST_INVALID", "AI 请求必须为单行 JSON")
        payload = json.loads(line)
        request = parse_request(payload)
        outputs = dispatch(request)
        emit_event(result(outputs))
        return 0
    except json.JSONDecodeError:
        emit_event(safe_error(WorkerError("AI_REQUEST_INVALID", "AI 请求不是有效 JSON")))
        return 2
    except BaseException as error:
        emit_event(safe_error(error))
        return 2


def configure_utf8_stdio() -> None:
    # Chinese Windows defaults to GBK/cp936 for piped stdio. Protocol JSON may
    # include non-ASCII paths; force UTF-8 so the desktop can read worker output.
    for stream in (sys.stdin, sys.stdout):
        reconfigure = getattr(stream, "reconfigure", None)
        if not callable(reconfigure):
            continue
        try:
            reconfigure(encoding="utf-8", errors="replace")
        except (OSError, ValueError):
            pass


def entrypoint(
    freeze_support: Callable[[], None] = multiprocessing.freeze_support,
    worker_main: Callable[[], int] = main,
    configure_stdio: Callable[[], None] = configure_utf8_stdio,
) -> int:
    freeze_support()
    configure_stdio()
    return worker_main()


if __name__ == "__main__":
    raise SystemExit(entrypoint())
