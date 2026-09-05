import hashlib
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
BUILDER = PROJECT_ROOT / "scripts" / "build-media-tools-from-source.sh"


class MediaToolBuilderTest(unittest.TestCase):
    def test_wrong_source_checksum_fails_before_creating_release_outputs(self):
        # Production mutation caught: compiling or publishing an FFmpeg source archive
        # whose bytes do not match the release-pinned provenance value.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "ffmpeg-source.tar.xz"
            payload = root / "payload"
            payload.mkdir()
            (payload / "README").write_text("fixture", encoding="utf-8")
            with tarfile.open(source, "w:xz") as bundle:
                bundle.add(payload, arcname="ffmpeg-fixture")
            x264 = root / "x264"
            (x264 / "include").mkdir(parents=True)
            (x264 / "lib" / "pkgconfig").mkdir(parents=True)
            (x264 / "lib" / "libx264.a").write_bytes(b"fixture")
            output = root / "output"
            actual = hashlib.sha256(source.read_bytes()).hexdigest()

            completed = subprocess.run(
                ["/bin/bash", str(BUILDER)],
                cwd=PROJECT_ROOT,
                env={
                    **os.environ,
                    "HONGGUO_FFMPEG_SOURCE_ARCHIVE": str(source),
                    "HONGGUO_FFMPEG_SOURCE_SHA256": "0" * 64,
                    "HONGGUO_X264_PREFIX": str(x264),
                    "HONGGUO_MEDIA_TOOLS_OUTPUT": str(output),
                },
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertNotEqual(actual, "0" * 64)
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("FFmpeg source SHA-256 mismatch", completed.stderr)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
