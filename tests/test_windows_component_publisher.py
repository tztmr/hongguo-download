import base64
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("publisher", Path(__file__).resolve().parents[1] / "scripts/publish-windows-components.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class PublisherGuardsTests(unittest.TestCase):
    def test_powershell_sizes_become_json_integers_without_truncation(self):
        size = publisher.byte_count(6028999054.0)
        self.assertIs(type(size), int)
        self.assertEqual(json.dumps(size), "6028999054")
        for invalid in (True, "123", 0, -1, 1.5, float("inf"), float("nan")):
            with self.subTest(value=invalid), self.assertRaises(AssertionError):
                publisher.byte_count(invalid)

    def test_rejects_failed_foreign_or_wrong_workflow_builds_before_publication(self):
        valid = {"status": "completed", "conclusion": "success", "path": ".github/workflows/windows-components.yml", "head_repository": {"full_name": "owner/repo"}}
        for override in ({"conclusion": "failure"}, {"status": "in_progress"},
                         {"path": ".github/workflows/untrusted.yml"}, {"head_repository": {"full_name": "foreign/repo"}}):
            with self.subTest(override=override), patch.dict("os.environ", {"GH_REPO": "owner/repo", "SOURCE_RUN_ID": "1", "COMPONENT_TAG": "windows-components-v4"}), \
                    patch.object(publisher.subprocess, "check_output", return_value=json.dumps({**valid, **override}).encode()), \
                    patch.object(publisher.subprocess, "run") as mutation:
                with self.assertRaises(AssertionError):
                    publisher.main()
                mutation.assert_not_called()

    def test_corrupt_artifact_is_not_uploaded(self):
        run = {"status": "completed", "conclusion": "success", "path": ".github/workflows/windows-components.yml", "head_repository": {"full_name": "owner/repo"}, "head_sha": "source"}
        manifest = {"components": [{"id": "runtime-cpu"}]}
        responses = [run, {"content": base64.b64encode(json.dumps(manifest).encode()).decode()},
                     [[{"id": 1, "tag_name": "windows-components-v4", "draft": True, "target_commitish": "source"}]]]

        def download(args, **_kwargs):
            self.assertEqual(args[:3], ["gh", "run", "download"])
            root = Path(args[args.index("--dir") + 1])
            (root / "hongguo-ai-runtime-windows-cpu.zip").write_bytes(b"corrupt")
            (root / "hongguo-ai-runtime-windows-cpu.json").write_text(json.dumps({"Flavor": "cpu", "Sha256": "0" * 64, "DownloadBytes": 7, "InstalledBytes": 10}))

        with patch.dict("os.environ", {"GH_REPO": "owner/repo", "SOURCE_RUN_ID": "1", "COMPONENT_TAG": "windows-components-v4"}), \
                patch.object(publisher.subprocess, "check_output", side_effect=[json.dumps(value).encode() for value in responses]), \
                patch.object(publisher.subprocess, "run", side_effect=download) as commands:
            with self.assertRaisesRegex(AssertionError, "checksum"):
                publisher.main()
            self.assertEqual(commands.call_count, 1)
