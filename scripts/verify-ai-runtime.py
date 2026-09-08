"""Run real separation through the frozen worker's JSON pipes before publishing."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import tarfile
import tempfile
import urllib.request
import wave


def download_models(cache: Path) -> list[tuple[str, Path]]:
    manifest = Path(__file__).resolve().parents[1] / "desktop/src-tauri/resources/ai-components.windows.json"
    models = []
    for item in json.loads(manifest.read_text(encoding="utf-8"))["components"]:
        if item["id"] not in {"demucs-htdemucs", "demucs-htdemucs_ft"}:
            continue
        root = cache / item["id"]
        root.mkdir(parents=True)
        archive = cache / (item["id"] + ".tar.gz")
        urllib.request.urlretrieve(item["url"], archive)
        with archive.open("rb") as stream:
            assert hashlib.file_digest(stream, "sha256").hexdigest() == item["sha256"], "model checksum mismatch"
        with tarfile.open(archive) as bundle:
            bundle.extractall(root, filter="data")
        model = item["id"].removeprefix("demucs-")
        bags = list(root.rglob(model + ".yaml"))
        assert len(bags) == 1, bags
        models.append((model, bags[0].parent))
    assert len(models) == 2
    return models


def verify(worker: Path, model: str, model_root: Path, root: Path) -> None:
    root.mkdir()
    source = root / "输入 音频.wav"
    output = root / "输出"
    output.mkdir()
    with wave.open(str(source), "wb") as audio:
        audio.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        audio.writeframes(b"".join(struct.pack("<h", int(5000 * math.sin(index * 0.08))) for index in range(16000)))

    def native(path: Path) -> str:
        value = str(path.resolve())
        return "\\\\?\\" + value if os.name == "nt" and not value.startswith("\\\\") else value

    request = {"version": 1, "jobId": "release-smoke", "operation": "separate",
               "inputPath": native(source), "outputDir": native(output),
               "options": {"model": model, "device": "cpu", "modelRoot": native(model_root)}}
    flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    completed = subprocess.run([str(worker)], input=(json.dumps(request) + "\n").encode(),
                               capture_output=True, timeout=600, creationflags=flags)
    events = []
    for line in completed.stdout.decode("utf-8").splitlines():
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    assert completed.returncode == 0, (events, completed.stderr.decode("utf-8", errors="replace")[-16000:])
    result = next(event for event in events if event.get("type") == "result")
    for key in ("vocalsPath", "backgroundMusicPath"):
        path = Path(result["outputs"][key]).resolve()
        assert path.is_relative_to(output.resolve()), path
        with wave.open(str(path), "rb") as audio:
            assert abs(audio.getnframes() / audio.getframerate() - 1) < 0.05
            assert audio.getnchannels() == 2
    print(f"Passed frozen {model} separation in a Unicode path with hidden console", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--worker", required=True, type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="红果 AI 验证 ") as directory:
        root = Path(directory)
        for model, model_root in download_models(root):
            verify(args.worker.resolve(), model, model_root, root / model)
