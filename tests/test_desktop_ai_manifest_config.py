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

    def test_windows_runtime_catalog_contains_publishable_assets(self):
        # Production mutation caught: shipping the 1-byte/all-zero placeholders
        # that make every Windows runtime download fail verification.
        manifest = json.loads(
            (
                PROJECT_ROOT
                / "desktop"
                / "src-tauri"
                / "resources"
                / "ai-components.windows.json"
            ).read_text(encoding="utf-8")
        )
        runtimes = {
            item["id"]: item
            for item in manifest["components"]
            if item["id"].startswith("runtime-")
        }
        self.assertEqual(
            set(runtimes), {"runtime-modern", "runtime-legacy", "runtime-cpu"}
        )
        for component_id, item in runtimes.items():
            with self.subTest(component=component_id):
                self.assertNotEqual(item["sha256"], "0" * 64)
                self.assertGreater(item["downloadBytes"], 1024 * 1024)
                self.assertGreater(item["installedBytes"], item["downloadBytes"])
                self.assertEqual(item["version"], "4")
                self.assertIn(
                    "/releases/download/windows-components-v4/", item["url"]
                )
                parts = item.get("parts") or []
                github_file_limit = 2_000_000_000
                if item["downloadBytes"] > github_file_limit:
                    self.assertGreaterEqual(len(parts), 2)
                    self.assertEqual(
                        sum(part["bytes"] for part in parts), item["downloadBytes"]
                    )
                    for index, part in enumerate(parts, start=1):
                        self.assertGreater(part["bytes"], 0)
                        self.assertLessEqual(part["bytes"], github_file_limit)
                        self.assertIn(
                            "/releases/download/windows-components-v4/",
                            part["url"],
                        )
                        self.assertTrue(
                            part["url"].endswith(f".zip.{index:03d}"),
                            part["url"],
                        )
                else:
                    self.assertEqual(parts, [])

    def test_install_command_runs_off_the_ui_thread(self):
        # Production mutation caught: keeping install_ai_component as a
        # synchronous Tauri command that freezes the window during download.
        source = (
            PROJECT_ROOT / "desktop" / "src-tauri" / "src" / "lib.rs"
        ).read_text(encoding="utf-8")
        self.assertIn("async fn install_ai_component(", source)
        self.assertIn("run_component_install(move ||", source)
        self.assertIn("async fn remove_ai_component(", source)
