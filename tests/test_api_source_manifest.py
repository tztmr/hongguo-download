import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from scripts.api_source_manifest import source_manifest


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which("node"), "Node is required by the desktop build")
class ApiSourceManifestTests(unittest.TestCase):
    def verify(self, root, manifest):
        return subprocess.run([
            "node", "--input-type=module", "-e",
            "import { verifySourceFiles } from "
            + json.dumps((ROOT / "scripts/verify-api-sidecar.mjs").as_uri())
            + "; import { readFileSync } from 'node:fs';"
            + " verifySourceFiles(process.argv[1], JSON.parse(readFileSync(0, 'utf8')));",
            str(root),
        ], input=json.dumps(manifest), text=True, capture_output=True)

    def test_rejects_old_api_when_heat_parser_changes_even_with_same_size_and_mtime(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for folder in ("core", "endpoints"):
                (root / folder).mkdir()
            for name in ("main.py", "requirements.txt", "requirements-build.txt"):
                (root / name).write_text("# fixture\n")
            parser = root / "core/feeds.py"
            parser.write_text("heat = None\n")
            old = source_manifest(root)
            self.assertEqual(self.verify(root, old).returncode, 0)
            stat = parser.stat()
            parser.write_text("heat = 1234\n")
            os.utime(parser, ns=(stat.st_atime_ns, stat.st_mtime_ns))
            failed = self.verify(root, old)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("core/feeds.py", failed.stderr)
            self.assertEqual(self.verify(root, source_manifest(root)).returncode, 0)

    def test_rejects_added_removed_sources_and_missing_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for folder in ("core", "endpoints"):
                (root / folder).mkdir()
            for name in ("main.py", "requirements.txt", "requirements-build.txt"):
                (root / name).write_text("# fixture\n")
            old = source_manifest(root)
            source = root / "endpoints/feed.py"
            source.write_text("# new endpoint\n")
            self.assertNotEqual(self.verify(root, old).returncode, 0)
            new = source_manifest(root)
            source.unlink()
            self.assertNotEqual(self.verify(root, new).returncode, 0)
            self.assertNotEqual(self.verify(root, {}).returncode, 0)
