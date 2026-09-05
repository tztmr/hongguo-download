from __future__ import annotations

import re
import stat
from pathlib import Path

import yaml

from ai_worker.protocol import WorkerError


_DEMUCS_SIGNATURE = re.compile(r"^[0-9a-f]{8}$")


def _regular_nonempty(path: Path) -> bool:
    try:
        metadata = path.lstat()
    except OSError:
        return False
    return not path.is_symlink() and stat.S_ISREG(metadata.st_mode) and metadata.st_size > 0


def validate_demucs_package(root: Path, model: str) -> None:
    bag = root / f"{model}.yaml"
    if not _regular_nonempty(bag):
        raise WorkerError("AI_MODEL_PACKAGE_INVALID", "Demucs 模型包不完整")
    try:
        document = yaml.safe_load(bag.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, yaml.YAMLError) as error:
        raise WorkerError("AI_MODEL_PACKAGE_INVALID", "Demucs 模型包不完整") from error
    signatures = document.get("models") if isinstance(document, dict) else None
    if (
        not isinstance(signatures, list)
        or not signatures
        or any(not isinstance(value, str) or not _DEMUCS_SIGNATURE.fullmatch(value) for value in signatures)
    ):
        raise WorkerError("AI_MODEL_PACKAGE_INVALID", "Demucs 模型包不完整")
    for signature in signatures:
        matches = [
            path
            for path in root.glob(f"{signature}*.th")
            if path.stem == signature or path.stem.startswith(f"{signature}-")
        ]
        if len(matches) != 1 or not _regular_nonempty(matches[0]):
            raise WorkerError("AI_MODEL_PACKAGE_INVALID", "Demucs 模型包不完整")


def validate_whisper_package(root: Path, model: str) -> Path:
    checkpoint = root / f"{model}.pt"
    if not _regular_nonempty(checkpoint):
        raise WorkerError("AI_MODEL_PACKAGE_INVALID", "Whisper 模型包不完整")
    return checkpoint
