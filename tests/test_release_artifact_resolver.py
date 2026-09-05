from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
RESOLVER = PROJECT_ROOT / "scripts" / "resolve-release-artifacts.sh"


class ReleaseArtifactResolverTest(unittest.TestCase):
    def test_ignores_stale_qa_bundles_and_returns_exact_product_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            bundle = Path(temporary)
            (bundle / "macos" / "红果下载 Feed QA.app").mkdir(parents=True)
            expected_app = bundle / "macos" / "红果下载.app"
            expected_app.mkdir()
            (bundle / "dmg").mkdir()
            (bundle / "dmg" / "红果下载_0.0.9_aarch64.dmg").write_bytes(b"old")
            expected_dmg = bundle / "dmg" / "红果下载_0.1.0_aarch64.dmg"
            expected_dmg.write_bytes(b"new")

            result = subprocess.run(
                [
                    "/bin/bash",
                    str(RESOLVER),
                    str(bundle),
                    "红果下载",
                    "0.1.0",
                    "aarch64",
                ],
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                result.stdout.splitlines(), [str(expected_app), str(expected_dmg)]
            )


if __name__ == "__main__":
    unittest.main()
