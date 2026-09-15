"""The real API lifespan must survive Windows' legacy redirected output encoding."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


class APIStartupTest(unittest.TestCase):
    def test_legacy_output_encoding_does_not_abort_http_startup(self):
        # Disable only remote device registration. HTTP binding and the complete
        # lifespan (including its Chinese startup banner) execute unchanged.
        script = '''import sys
from unittest.mock import AsyncMock
import main, uvicorn
main.scheduler.start = AsyncMock()
main.scheduler.stop = AsyncMock()
uvicorn.run(main.app, host="127.0.0.1", port=int(sys.argv[1]), log_level="warning")
'''
        with tempfile.TemporaryDirectory(prefix="hongguo-stdio-test-") as directory:
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            env = {**os.environ, "HONGGUO_DATA_DIR": directory, "PYTHONIOENCODING": "cp1252"}
            log_path = Path(directory) / "server.log"
            ready = None
            with log_path.open("wb") as log:
                child = subprocess.Popen([sys.executable, "-B", "-c", script, str(port)],
                                         cwd=ROOT, env=env, stdout=log, stderr=log)
                try:
                    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                    deadline = time.monotonic() + 8
                    while time.monotonic() < deadline and child.poll() is None:
                        try:
                            with opener.open(f"http://127.0.0.1:{port}/health", timeout=.3) as response:
                                ready = json.load(response)
                            break
                        except (OSError, ValueError):
                            time.sleep(.05)
                finally:
                    child.terminate()
                    try:
                        child.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=3)
            diagnostic = log_path.read_text(encoding="utf-8", errors="replace")
            self.assertIsNotNone(ready, diagnostic)
            self.assertEqual(ready["api_contract"], "hongguo-desktop-v2")
            self.assertNotIn("UnicodeEncodeError", diagnostic)
