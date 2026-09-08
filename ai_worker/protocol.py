from __future__ import annotations

import json
import os
import re
import stat
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, TextIO


REQUEST_FIELDS = {"version", "jobId", "operation", "inputPath", "outputDir", "options"}
JOB_ID = re.compile(r"^[A-Za-z0-9_-]{1,128}$")


class WorkerError(Exception):
    def __init__(self, code: str, message: str):
        self.code = code
        self.message = message
        super().__init__(f"{code}: {message}")


@dataclass(frozen=True)
class WorkerRequest:
    version: int
    job_id: str
    operation: str
    input_path: Path
    output_dir: Path
    options: dict[str, Any]


def _invalid(message: str = "AI 请求无效") -> WorkerError:
    return WorkerError("AI_REQUEST_INVALID", message)


def _is_regular_without_links(path: Path) -> bool:
    try:
        info = path.lstat()
    except OSError:
        return False
    return stat.S_ISREG(info.st_mode) and not path.is_symlink()


def _is_directory_without_links(path: Path) -> bool:
    try:
        info = path.lstat()
    except OSError:
        return False
    return stat.S_ISDIR(info.st_mode) and not path.is_symlink()


def parse_request(payload: Mapping[str, Any]) -> WorkerRequest:
    if not isinstance(payload, Mapping) or set(payload) != REQUEST_FIELDS:
        raise _invalid()
    if payload.get("version") != 1:
        raise _invalid("不支持的 AI 请求版本")
    job_id = payload.get("jobId")
    operation = payload.get("operation")
    options = payload.get("options")
    if not isinstance(job_id, str) or not JOB_ID.fullmatch(job_id):
        raise _invalid()
    if operation not in {"separate", "transcribe"}:
        raise _invalid()
    if not isinstance(options, dict):
        raise _invalid()
    try:
        input_path = Path(os.fspath(payload.get("inputPath")))
        output_dir = Path(os.fspath(payload.get("outputDir")))
    except TypeError as error:
        raise _invalid() from error
    if not input_path.is_absolute() or not output_dir.is_absolute():
        raise _invalid()
    if not _is_regular_without_links(input_path) or not _is_directory_without_links(output_dir):
        raise _invalid()
    input_parent = input_path.parent.resolve(strict=True)
    resolved_input = input_path.resolve(strict=True)
    resolved_output = output_dir.resolve(strict=True)
    if resolved_input.parent != input_parent or not resolved_output.is_relative_to(input_parent):
        raise _invalid()
    return WorkerRequest(1, job_id, operation, resolved_input, resolved_output, dict(options))


def progress(stage: str, percent: float) -> dict[str, Any]:
    return {"type": "progress", "stage": stage, "percent": max(0.0, min(100.0, float(percent)))}


def result(outputs: Mapping[str, Any]) -> dict[str, Any]:
    return {"type": "result", "outputs": dict(outputs)}


def safe_error(error: BaseException) -> dict[str, str]:
    if isinstance(error, WorkerError):
        return {"type": "error", "code": error.code, "message": error.message}
    return {"type": "error", "code": "AI_WORKER_FAILED", "message": "AI 处理失败"}


def emit_event(event: Mapping[str, Any], stream: TextIO | None = None) -> None:
    stream = sys.stdout if stream is None else stream
    stream.write(json.dumps(dict(event), ensure_ascii=True, separators=(",", ":")) + "\n")
    stream.flush()
