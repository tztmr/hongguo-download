import base64
import hashlib
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
        run = {"status": "completed", "conclusion": "success", "path": ".github/workflows/windows-components.yml", "head_repository": {"full_name": "owner/repo"}, "head_sha": "source", "html_url": "https://example.test/run/1"}
        manifest = {"components": [{"id": "runtime-cpu"}]}
        responses = [run, {"visibility": "public", "private": False, "default_branch": "main"}, {"sha": "public-head"},
                     {"content": base64.b64encode(json.dumps(manifest).encode()).decode()},
                     [[{"id": 1, "tag_name": "windows-components-v4", "draft": True, "body": "Source: owner/repo@source. Build: https://example.test/run/1"}]]]

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

    def test_rejects_private_download_repository_before_any_mutation(self):
        run = {"status": "completed", "conclusion": "success", "path": ".github/workflows/windows-components.yml", "head_repository": {"full_name": "owner/repo"}, "head_sha": "source"}
        with patch.dict("os.environ", {"GH_REPO": "owner/repo", "SOURCE_RUN_ID": "1", "COMPONENT_TAG": "windows-components-v4"}), \
                patch.object(publisher.subprocess, "check_output", side_effect=[json.dumps(value).encode() for value in [run, {"visibility": "private", "private": True}]]), \
                patch.object(publisher.subprocess, "run") as mutation:
            with self.assertRaisesRegex(AssertionError, "public release repository"):
                publisher.main()
            mutation.assert_not_called()

    def test_downloads_from_source_and_publishes_only_to_public_repository(self):
        public = "owner/public-components"
        provenance = "Source: owner/repo@source. Build: https://example.test/run/1"
        manifest = {"components": [{"id": f"runtime-{flavor}"} for flavor in ("cpu", "legacy", "modern")]}
        draft = {"id": 1, "tag_name": "windows-components-v4", "draft": True, "body": provenance, "assets": []}
        commands = []
        uploaded = {}

        def api(args, **_kwargs):
            endpoint = args[-1]
            if endpoint == "repos/owner/repo/actions/runs/1":
                value = {"status": "completed", "conclusion": "success", "path": ".github/workflows/windows-components.yml", "head_repository": {"full_name": "owner/repo"}, "head_sha": "source", "html_url": "https://example.test/run/1"}
            elif endpoint == f"repos/{public}":
                value = {"visibility": "public", "private": False, "default_branch": "public-main"}
            elif endpoint == f"repos/{public}/commits/public-main":
                value = {"sha": "public-head"}
            elif endpoint.startswith("repos/owner/repo/contents/"):
                value = {"content": base64.b64encode(json.dumps(manifest).encode()).decode()}
            elif endpoint == f"repos/{public}/releases?per_page=100":
                created = any(command[:3] == ["gh", "release", "create"] for command in commands)
                value = [[draft]] if created else [[]]
            elif endpoint == f"repos/{public}/releases/1":
                value = draft
            else:
                self.fail(endpoint)
            return json.dumps(value).encode()

        def command(args, **_kwargs):
            commands.append(args)
            if args[:3] == ["gh", "run", "download"]:
                self.assertEqual(args[args.index("--repo") + 1], "owner/repo")
                root = Path(args[args.index("--dir") + 1])
                stem = args[args.index("--name") + 1]
                archive = b"verified archive"
                (root / f"{stem}.zip").write_bytes(archive)
                (root / f"{stem}.json").write_text(json.dumps({"Flavor": stem.split("-")[-1], "Sha256": hashlib.sha256(archive).hexdigest(), "DownloadBytes": len(archive), "InstalledBytes": 100.0}))
            else:
                self.assertEqual(args[args.index("--repo") + 1], public)
                if args[2] == "create":
                    self.assertEqual(args[args.index("--target") + 1], "public-head")
                if args[2] == "upload":
                    path = Path(args[4])
                    uploaded[path.name] = path.read_bytes()

        with patch.dict("os.environ", {"GH_REPO": "owner/repo", "COMPONENT_RELEASE_REPO": public, "SOURCE_RUN_ID": "1", "COMPONENT_TAG": "windows-components-v4"}), \
                patch.object(publisher.subprocess, "check_output", side_effect=api), \
                patch.object(publisher.subprocess, "run", side_effect=command):
            publisher.main()
        self.assertEqual(len(uploaded), 4)
        for item in json.loads(uploaded["ai-components.windows.json"])["components"]:
            self.assertTrue(item["url"].startswith(f"https://github.com/{public}/releases/download/"))
            self.assertIs(type(item["installedBytes"]), int)
        self.assertEqual(commands[-1][2], "edit")
