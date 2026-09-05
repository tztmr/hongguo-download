import json
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
BUILDER = PROJECT_ROOT / "scripts" / "build-ai-model-archives.py"


def create_model_inputs(root: Path) -> tuple[Path, Path, Path]:
    demucs = root / "demucs"
    demucs.mkdir()
    (demucs / "htdemucs.yaml").write_text(
        "models: ['955717e8']\n", encoding="utf-8"
    )
    (demucs / "htdemucs_ft.yaml").write_text(
        "models: ['f7e0c4bc', 'd12395a8', '92cfc3b6', '04573f0d']\n",
        encoding="utf-8",
    )
    for signature in (
        "955717e8-a1b2c3d4",
        "f7e0c4bc-11111111",
        "d12395a8-22222222",
        "92cfc3b6-33333333",
        "04573f0d-44444444",
    ):
        (demucs / f"{signature}.th").write_bytes(signature.encode("ascii"))
    small = root / "official-small-checkpoint.pt"
    medium = root / "official-medium-checkpoint.pt"
    small.write_bytes(b"small-weights")
    medium.write_bytes(b"medium-weights")
    return demucs, small, medium


def run_builder(demucs: Path, small: Path, medium: Path, output: Path):
    return subprocess.run(
        [
            sys.executable,
            str(BUILDER),
            "--demucs-dir",
            str(demucs),
            "--whisper-small",
            str(small),
            "--whisper-medium",
            str(medium),
            "--output-dir",
            str(output),
        ],
        cwd=PROJECT_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


class AIModelArchiveBuilderTest(unittest.TestCase):
    def test_builds_four_installable_archives_and_machine_readable_metadata(self):
        # Production mutation caught: producing archives with renamed or missing files
        # that disagree with the runtime's fixed component entrypoints.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            demucs, small, medium = create_model_inputs(root)
            output = root / "output"

            completed = run_builder(demucs, small, medium, output)

            self.assertEqual(completed.returncode, 0, completed.stderr)
            metadata = json.loads(
                (output / "release-metadata.json").read_text(encoding="utf-8")
            )
            self.assertEqual(
                set(metadata["components"]),
                {
                    "demucs-htdemucs",
                    "demucs-htdemucs_ft",
                    "whisper-small",
                    "whisper-medium",
                },
            )
            expected_members = {
                "demucs-htdemucs": {
                    "htdemucs.yaml",
                    "955717e8-a1b2c3d4.th",
                },
                "demucs-htdemucs_ft": {
                    "htdemucs_ft.yaml",
                    "f7e0c4bc-11111111.th",
                    "d12395a8-22222222.th",
                    "92cfc3b6-33333333.th",
                    "04573f0d-44444444.th",
                },
                "whisper-small": {"small.pt"},
                "whisper-medium": {"medium.pt"},
            }
            for component_id, members in expected_members.items():
                item = metadata["components"][component_id]
                archive = output / item["archive"]
                self.assertEqual(len(item["sha256"]), 64)
                self.assertEqual(item["downloadBytes"], archive.stat().st_size)
                with tarfile.open(archive, "r:gz") as bundle:
                    self.assertEqual(set(bundle.getnames()), members)

    def test_missing_referenced_weight_fails_before_any_archive_is_published(self):
        # Production mutation caught: publishing a partial Demucs bag that only fails
        # when a user starts a long-running separation task.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            demucs, small, medium = create_model_inputs(root)
            (demucs / "04573f0d-44444444.th").unlink()
            output = root / "output"

            completed = run_builder(demucs, small, medium, output)

            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("missing Demucs weight 04573f0d", completed.stderr)
            self.assertFalse(output.exists())

    def test_symlinked_checkpoint_is_not_followed_into_a_release_archive(self):
        # Production mutation caught: resolving an input before lstat and silently
        # packaging a checkpoint reached through an unsafe symbolic link.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            demucs, small, medium = create_model_inputs(root)
            linked = root / "small-link.pt"
            linked.symlink_to(small)
            output = root / "output"

            completed = run_builder(demucs, linked, medium, output)

            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("regular files", completed.stderr)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
