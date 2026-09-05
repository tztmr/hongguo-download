#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
import gzip
import hashlib
import json
import os
import re
import stat
import sys
import tarfile
import tempfile
from pathlib import Path


SIGNATURE = re.compile(r"^[0-9a-f]{8}$")


def regular_nonempty(path: Path) -> bool:
    try:
        metadata = path.lstat()
    except OSError:
        return False
    return not path.is_symlink() and stat.S_ISREG(metadata.st_mode) and metadata.st_size > 0


def demucs_files(root: Path, model: str) -> list[Path]:
    bag = root / f"{model}.yaml"
    if not regular_nonempty(bag):
        raise ValueError(f"missing Demucs bag {model}.yaml")
    try:
        models_line = next(
            line.split(":", 1)[1].strip()
            for line in bag.read_text(encoding="utf-8").splitlines()
            if line.strip().startswith("models:")
        )
        signatures = ast.literal_eval(models_line)
    except (OSError, UnicodeError, StopIteration, SyntaxError, ValueError) as error:
        raise ValueError(f"invalid Demucs bag {model}.yaml") from error
    if (
        not isinstance(signatures, list)
        or not signatures
        or any(not isinstance(value, str) or not SIGNATURE.fullmatch(value) for value in signatures)
    ):
        raise ValueError(f"invalid Demucs bag {model}.yaml")
    files = [bag]
    for signature in signatures:
        matches = [
            path
            for path in root.glob(f"{signature}*.th")
            if (path.stem == signature or path.stem.startswith(f"{signature}-"))
            and regular_nonempty(path)
        ]
        if len(matches) != 1:
            raise ValueError(f"missing Demucs weight {signature}")
        files.append(matches[0])
    return files


def add_file(bundle: tarfile.TarFile, source: Path, archive_name: str) -> None:
    metadata = source.stat()
    info = tarfile.TarInfo(archive_name)
    info.size = metadata.st_size
    info.mode = 0o644
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    with source.open("rb") as stream:
        bundle.addfile(info, stream)


def build_archive(destination: Path, files: list[tuple[Path, str]]) -> None:
    temporary = destination.with_suffix(destination.suffix + ".tmp")
    with temporary.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as bundle:
                for source, archive_name in files:
                    add_file(bundle, source, archive_name)
        raw.flush()
        os.fsync(raw.fileno())
    os.replace(temporary, destination)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Build offline AI model component archives")
    parser.add_argument("--demucs-dir", required=True, type=Path)
    parser.add_argument("--whisper-small", required=True, type=Path)
    parser.add_argument("--whisper-medium", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if not regular_nonempty(args.whisper_small) or not regular_nonempty(
            args.whisper_medium
        ):
            raise ValueError("Whisper checkpoints must be nonempty regular files")
        demucs_root = args.demucs_dir.resolve(strict=True)
        small = args.whisper_small.resolve(strict=True)
        medium = args.whisper_medium.resolve(strict=True)
        if not demucs_root.is_dir():
            raise ValueError("Demucs source is not a directory")
        components = {
            "demucs-htdemucs": (
                "htdemucs.yaml",
                [(path, path.name) for path in demucs_files(demucs_root, "htdemucs")],
            ),
            "demucs-htdemucs_ft": (
                "htdemucs_ft.yaml",
                [(path, path.name) for path in demucs_files(demucs_root, "htdemucs_ft")],
            ),
            "whisper-small": ("small.pt", [(small, "small.pt")]),
            "whisper-medium": ("medium.pt", [(medium, "medium.pt")]),
        }
        output = args.output_dir.resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="hongguo-ai-models-", dir=output.parent) as temporary:
            staging = Path(temporary)
            metadata: dict[str, dict[str, str | int]] = {}
            for component_id, (entrypoint, files) in components.items():
                archive_name = f"{component_id}.tar.gz"
                archive = staging / archive_name
                build_archive(archive, files)
                metadata[component_id] = {
                    "archive": archive_name,
                    "sha256": sha256_file(archive),
                    "downloadBytes": archive.stat().st_size,
                    "installedBytes": sum(path.stat().st_size for path, _ in files),
                    "entrypoint": entrypoint,
                }
            metadata_path = staging / "release-metadata.json"
            metadata_path.write_text(
                json.dumps({"components": metadata}, ensure_ascii=False, indent=2) + "\n",
                encoding="utf-8",
            )
            output.mkdir(parents=True, exist_ok=True)
            for path in staging.iterdir():
                os.replace(path, output / path.name)
        print(output / "release-metadata.json")
        return 0
    except (OSError, ValueError) as error:
        print(f"AI model archive build: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
