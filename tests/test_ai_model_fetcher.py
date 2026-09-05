from __future__ import annotations

import io
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
FETCHER = PROJECT_ROOT / "scripts" / "fetch-ai-model-sources.sh"


class AiModelFetcherTest(unittest.TestCase):
    def test_checksum_failure_does_not_publish_download(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runtime = root / "runtime.tar.gz"
            with tarfile.open(runtime, "w:gz") as archive:
                for name, content in (
                    ("htdemucs.yaml", b"models: ['955717e8']\n"),
                    (
                        "htdemucs_ft.yaml",
                        b"models: ['f7e0c4bc', 'd12395a8', '92cfc3b6', '04573f0d']\n",
                    ),
                ):
                    data = io.BytesIO(content)
                    info = tarfile.TarInfo(
                        f"hongguo-ai-worker/_internal/demucs/remote/{name}"
                    )
                    info.size = len(content)
                    archive.addfile(info, data)

            fake_curl = root / "curl"
            fake_curl.write_text(
                "#!/bin/bash\nprintf 'wrong payload' > \"${@: -1}\"\n",
                encoding="utf-8",
            )
            fake_curl.chmod(0o755)
            output = root / "models"
            result = subprocess.run(
                ["/bin/bash", str(FETCHER)],
                cwd=PROJECT_ROOT,
                env={
                    **os.environ,
                    "HONGGUO_AI_RUNTIME_ARCHIVE": str(runtime),
                    "HONGGUO_AI_MODEL_SOURCE_DIR": str(output),
                    "HONGGUO_CURL_BIN": str(fake_curl),
                },
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("model SHA-256 mismatch", result.stderr)
            self.assertFalse((output / "955717e8-8726e21a.th").exists())


if __name__ == "__main__":
    unittest.main()
