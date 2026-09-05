import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
RENDER_SCRIPT = PROJECT_ROOT / "scripts" / "render-ai-component-manifest.sh"


def release_environment():
    return {
        **os.environ,
        "HONGGUO_AI_RUNTIME_URL": "https://github.com/hongguo-fixtures/releases/download/v1/runtime.tar",
        "HONGGUO_AI_RUNTIME_SHA256": "1" * 64,
        "HONGGUO_AI_RUNTIME_DOWNLOAD_BYTES": "100",
        "HONGGUO_AI_RUNTIME_INSTALLED_BYTES": "200",
        "HONGGUO_AI_RUNTIME_VERSION": "1",
        "HONGGUO_AI_DEMUCS_URL": "https://github.com/hongguo-fixtures/releases/download/v1/htdemucs.tar",
        "HONGGUO_AI_DEMUCS_SHA256": "2" * 64,
        "HONGGUO_AI_DEMUCS_DOWNLOAD_BYTES": "100",
        "HONGGUO_AI_DEMUCS_INSTALLED_BYTES": "200",
        "HONGGUO_AI_DEMUCS_VERSION": "1",
        "HONGGUO_AI_DEMUCS_FT_URL": "https://github.com/hongguo-fixtures/releases/download/v1/htdemucs-ft.tar",
        "HONGGUO_AI_DEMUCS_FT_SHA256": "3" * 64,
        "HONGGUO_AI_DEMUCS_FT_DOWNLOAD_BYTES": "100",
        "HONGGUO_AI_DEMUCS_FT_INSTALLED_BYTES": "200",
        "HONGGUO_AI_DEMUCS_FT_VERSION": "1",
        "HONGGUO_AI_WHISPER_URL": "https://github.com/hongguo-fixtures/releases/download/v1/small.tar",
        "HONGGUO_AI_WHISPER_SHA256": "4" * 64,
        "HONGGUO_AI_WHISPER_DOWNLOAD_BYTES": "100",
        "HONGGUO_AI_WHISPER_INSTALLED_BYTES": "200",
        "HONGGUO_AI_WHISPER_VERSION": "1",
        "HONGGUO_AI_WHISPER_MEDIUM_URL": "https://github.com/hongguo-fixtures/releases/download/v1/medium.tar",
        "HONGGUO_AI_WHISPER_MEDIUM_SHA256": "5" * 64,
        "HONGGUO_AI_WHISPER_MEDIUM_DOWNLOAD_BYTES": "100",
        "HONGGUO_AI_WHISPER_MEDIUM_INSTALLED_BYTES": "200",
        "HONGGUO_AI_WHISPER_MEDIUM_VERSION": "1",
    }


class AIReleaseManifestTest(unittest.TestCase):
    def test_invalid_download_urls_never_overwrite_an_existing_manifest(self):
        # Catches accepting an HTTPS prefix without a real public download host.
        urls = (
            "https://downloads.invalid/runtime.tar.gz",
            "https://DOWNLOADS.INVALID./runtime.tar.gz",
            "https://cdn.example.com/runtime.tar.gz",
            "https://example.net/runtime.tar.gz",
            "https://models.test/runtime.tar.gz",
            "https://localhost/runtime.tar.gz",
            "https://127.0.0.1/runtime.tar.gz",
            "https://[::1]/runtime.tar.gz",
            "https://192.168.1.2/runtime.tar.gz",
            "https:///runtime.tar.gz",
            "https://github.com:bad/runtime.tar.gz",
            "https://github.com\\@downloads.invalid/runtime.tar.gz",
            "https://github.com/runtime.tar.gz#fragment",
            "https://github.com/a\nb.tar.gz",
            "https://user:private-password@github.com/runtime.tar.gz",
        )
        for url in urls:
            with self.subTest(url=url), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "ai-components.json"
                output.write_text("existing manifest", encoding="utf-8")
                completed = subprocess.run(
                    ["/bin/bash", str(RENDER_SCRIPT), str(output)],
                    env={**release_environment(), "HONGGUO_AI_RUNTIME_URL": url},
                    text=True, capture_output=True, check=False,
                )
                self.assertNotEqual(completed.returncode, 0, completed.stdout)
                self.assertIn("AI component URL", completed.stderr)
                self.assertNotIn("private-password", completed.stderr)
                self.assertEqual(output.read_text(), "existing manifest")

    def test_every_component_url_is_checked(self):
        for prefix in ("DEMUCS", "DEMUCS_FT", "WHISPER", "WHISPER_MEDIUM"):
            with self.subTest(component=prefix), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "new" / "ai-components.json"
                completed = subprocess.run(
                    ["/bin/bash", str(RENDER_SCRIPT), str(output)],
                    env={**release_environment(), f"HONGGUO_AI_{prefix}_URL": "https://downloads.invalid/model.tar.gz"},
                    text=True, capture_output=True, check=False,
                )
                self.assertNotEqual(completed.returncode, 0)
                self.assertFalse(output.parent.exists())

    def test_release_build_rejects_placeholder_before_starting_sidecar_build(self):
        # Catches validation that runs only after expensive build/staging writes.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scripts = root / "scripts"
            shutil.copytree(PROJECT_ROOT / "scripts", scripts)
            sentinel = scripts / "build-api-sidecar.sh"
            sentinel.write_text('#!/bin/bash\ntouch build-started\nexit 91\n')
            archive = root / "tools.tar.gz"
            archive.write_bytes(b"fixture")
            env = {
                **release_environment(),
                "HONGGUO_AI_WHISPER_MEDIUM_URL": "https://downloads.invalid/medium.tar.gz",
                "HONGGUO_FFMPEG_ARCHIVE": str(archive),
                "HONGGUO_FFMPEG_SHA256": "a" * 64,
                "HONGGUO_FFPROBE_SHA256": "b" * 64,
            }
            completed = subprocess.run(
                ["/bin/bash", str(scripts / "build-release.sh")],
                env=env, text=True, capture_output=True, check=False,
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("AI component URL", completed.stderr)
            self.assertFalse((root / "build-started").exists())

    def test_model_entrypoints_match_the_files_consumed_by_offline_runtimes(self):
        # Production mutation caught: using a generic marker as the entrypoint while
        # Whisper and Demucs expect specifically named files in the component root.
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "ai-components.json"
            env = release_environment()
            completed = subprocess.run(
                ["/bin/bash", str(RENDER_SCRIPT), str(output)],
                cwd=PROJECT_ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertEqual(completed.returncode, 0, completed.stderr)
            manifest = json.loads(output.read_text(encoding="utf-8"))
            entrypoints = {
                item["id"]: item["entrypoint"] for item in manifest["components"]
            }
            self.assertEqual(
                entrypoints,
                {
                    "runtime": "hongguo-ai-worker/hongguo-ai-worker",
                    "demucs-htdemucs": "htdemucs.yaml",
                    "demucs-htdemucs_ft": "htdemucs_ft.yaml",
                    "whisper-small": "small.pt",
                    "whisper-medium": "medium.pt",
                },
            )


if __name__ == "__main__":
    unittest.main()
