#!/usr/bin/env python3
"""Check the installed native app -> bundled API -> HTTP readiness path."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def verify(executable: Path, version: str, timeout: float = 90):
    with tempfile.TemporaryDirectory(prefix="hongguo-app-startup-") as temporary:
        root = Path(temporary)
        data = root / "隔离 启动验证"
        env = os.environ.copy()
        if os.name != "nt":
            env["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
        with (root / "native.log").open("wb") as log:
            child = subprocess.Popen(
                [str(executable.resolve()), "--startup-probe", str(data)],
                env=env, stdout=log, stderr=log, start_new_session=os.name != "nt",
            )
            started = time.monotonic()
            try:
                while time.monotonic() - started < timeout:
                    report_path = data / "result.json"
                    if report_path.exists():
                        try:
                            report = json.loads(report_path.read_text(encoding="utf-8"))
                        except json.JSONDecodeError:
                            time.sleep(.1)
                            continue
                        if report.get("status") != "ok" or report.get("version") != version:
                            raise RuntimeError(f"Native startup verification failed: {report}")
                        print(f"Native app startup passed: v{version}, bundled API connected in {time.monotonic() - started:.1f}s")
                        return
                    if child.poll() is not None:
                        raise RuntimeError(f"Native app exited before API readiness: {child.returncode}")
                    time.sleep(.1)
                raise RuntimeError("Native app API startup timed out")
            finally:
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(child.pid), "/T", "/F"],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
                else:
                    try:
                        os.killpg(child.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    if os.name != "nt":
                        os.killpg(child.pid, signal.SIGKILL)
                    child.kill()
                    child.wait(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("version")
    arguments = parser.parse_args()
    verify(arguments.executable, arguments.version)
