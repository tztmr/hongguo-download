"""Verify real decoded-audio progress through a frozen worker's JSON pipe.

Pass a spoken WAV longer than 30 seconds and an existing offline small model.
The original WAV and model are read only; outputs live in a temporary directory.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time


def verify(worker: Path, audio: Path, model_root: Path) -> list[dict]:
    with tempfile.TemporaryDirectory(prefix="hongguo-subtitle-progress-") as directory:
        root = Path(directory)
        source, output = root / "dialogue.wav", root / "output"
        shutil.copyfile(audio, source)
        output.mkdir()
        request = {"version": 1, "jobId": "subtitle-progress-smoke", "operation": "transcribe",
                   "inputPath": str(source), "outputDir": str(output),
                   "options": {"model": "small", "device": "cpu", "language": "zh", "modelRoot": str(model_root)}}
        environment = {**os.environ, "OMP_NUM_THREADS": "2", "MKL_NUM_THREADS": "2", "VECLIB_MAXIMUM_THREADS": "2"}
        with (root / "diagnostics.log").open("w") as diagnostics:
            process = subprocess.Popen([str(worker)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=diagnostics, text=True, encoding="utf-8", env=environment)
            timeout = threading.Timer(600, process.kill)
            timeout.start()
            started = time.monotonic()
            events = []
            try:
                process.stdin.write(json.dumps(request) + "\n")
                process.stdin.close()
                for line in process.stdout:
                    event = json.loads(line)
                    events.append(event)
                    if event["type"] == "progress":
                        print(json.dumps({**event, "elapsed": round(time.monotonic() - started, 2)}, ensure_ascii=False), flush=True)
                assert process.wait() == 0, "Frozen transcription failed"
                percents = [event["percent"] for event in events if event["type"] == "progress"]
                assert any(10 < value < 85 for value in percents), "No intermediate transcription progress"
                assert percents == sorted(percents) and percents[-1] == 100, percents
                result = next(event["outputs"] for event in events if event["type"] == "result")
                srt = Path(result["srtPath"]).read_text(encoding="utf-8")
                assert " --> " in srt and result["segmentCount"] > 0, "Missing recognized subtitles"
                print(json.dumps({"passed": True, "segmentCount": result["segmentCount"], "language": result["language"]}), flush=True)
                return events
            finally:
                timeout.cancel()
                if process.poll() is None:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--audio", type=Path, required=True)
    parser.add_argument("--model-root", type=Path, required=True)
    args = parser.parse_args()
    verify(args.worker.resolve(), args.audio.resolve(), args.model_root.resolve())
