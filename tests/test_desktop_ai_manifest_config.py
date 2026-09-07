import json
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]


class DesktopAIManifestConfigTest(unittest.TestCase):
    def test_development_and_release_configs_package_the_ai_manifest(self):
        for name in ("tauri.conf.json", "tauri.release.conf.json"):
            with self.subTest(config=name):
                config = json.loads(
                    (PROJECT_ROOT / "desktop" / "src-tauri" / name).read_text(
                        encoding="utf-8"
                    )
                )
                resources = config["bundle"]["resources"]
                self.assertEqual(
                    resources.get("resources/ai-components.json"),
                    "resources/ai-components.json",
                )

