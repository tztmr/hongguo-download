import json
import os
import plistlib
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
VERIFY_SCRIPT = PROJECT_ROOT / "scripts" / "verify-release.sh"


def create_release_fixture(root: Path) -> tuple[Path, Path, Path]:
    app = root / "Fixture.app"
    macos = app / "Contents" / "MacOS"
    resources = app / "Contents" / "Resources"
    macos.mkdir(parents=True)
    resources.mkdir(parents=True)
    source = root / "fixture.c"
    source.write_text("int main(void) { return 0; }\n", encoding="utf-8")
    executable = macos / "Fixture"
    subprocess.run(
        ["/usr/bin/clang", str(source), "-o", str(executable)],
        check=True,
        capture_output=True,
        text=True,
    )
    for name in (
        "hongguo-api-aarch64-apple-darwin",
        "ffmpeg-aarch64-apple-darwin",
        "ffprobe-aarch64-apple-darwin",
    ):
        shutil.copy2(executable, resources / name)
    (resources / "THIRD_PARTY_NOTICES.md").write_text(
        "fixture licenses\n", encoding="utf-8"
    )
    manifest = resources / "ai-components.json"
    components = [
        ("runtime", "hongguo-ai-worker/hongguo-ai-worker"),
        ("demucs-htdemucs", "htdemucs.yaml"),
        ("demucs-htdemucs_ft", "htdemucs_ft.yaml"),
        ("whisper-small", "small.pt"),
        ("whisper-medium", "medium.pt"),
    ]
    manifest.write_text(
        json.dumps(
            {
                "version": 1,
                "platform": "aarch64-apple-darwin",
                "components": [
                    {
                        "id": component_id,
                        "version": "1",
                        "platform": "aarch64-apple-darwin",
                        "url": f"https://github.com/hongguo-fixtures/releases/download/v1/{component_id}.tar",
                        "sha256": str(index) * 64,
                        "downloadBytes": 1,
                        "installedBytes": 1,
                        "entrypoint": entrypoint,
                    }
                    for index, (component_id, entrypoint) in enumerate(components, 1)
                ],
            }
        ),
        encoding="utf-8",
    )
    dmg = root / "Fixture.dmg"
    dmg.write_bytes(b"fixture")
    return app, dmg, manifest


def sign_and_create_dmg(app: Path, dmg: Path, executable_name: str = "Fixture") -> None:
    original_executable = app / "Contents" / "MacOS" / "Fixture"
    executable = app / "Contents" / "MacOS" / executable_name
    if executable != original_executable:
        original_executable.rename(executable)
    with (app / "Contents" / "Info.plist").open("wb") as stream:
        plistlib.dump(
            {
                "CFBundleExecutable": executable_name,
                "CFBundleIdentifier": "com.edking.hongguo.fixture",
                "CFBundleName": "Fixture",
                "CFBundlePackageType": "APPL",
                "CFBundleVersion": "1",
            },
            stream,
        )
    subprocess.run(
        ["/usr/bin/codesign", "--force", "--deep", "--sign", "-", str(app)],
        check=True,
        capture_output=True,
        text=True,
    )
    dmg.unlink()
    subprocess.run(
        [
            "/usr/bin/hdiutil",
            "create",
            "-srcfolder",
            str(app),
            "-format",
            "UDZO",
            str(dmg),
        ],
        check=True,
        capture_output=True,
        text=True,
    )


def run_verifier(app: Path, dmg: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["/bin/bash", str(VERIFY_SCRIPT), str(app), str(dmg)],
        cwd=PROJECT_ROOT,
        env=os.environ.copy(),
        text=True,
        capture_output=True,
        check=False,
        timeout=10,
    )


class ReleaseVerifierTest(unittest.TestCase):
    def test_rejects_placeholder_urls_in_an_existing_packaged_manifest(self):
        # Catches shipping a structurally valid manifest with unusable install URLs.
        with tempfile.TemporaryDirectory() as directory:
            app, dmg, manifest_path = create_release_fixture(Path(directory))
            manifest = json.loads(manifest_path.read_text())
            manifest["components"][-1]["url"] = "https://downloads.invalid/medium.tar.gz"
            manifest_path.write_text(json.dumps(manifest))
            completed = run_verifier(app, dmg)
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("AI component URL", completed.stderr)

    def test_valid_signed_synthetic_bundle_and_dmg_pass_the_complete_verifier(self):
        # Production mutation caught: a verifier whose independent checks can reject
        # every fixture but can never accept a correctly composed signed release.
        with tempfile.TemporaryDirectory() as directory:
            app, dmg, _ = create_release_fixture(Path(directory))
            sign_and_create_dmg(app, dmg)

            completed = run_verifier(app, dmg)

        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        self.assertIn("Fixture.app/Contents/MacOS/Fixture", completed.stdout)
        self.assertIn("Fixture.dmg", completed.stdout)

    def test_hashes_cf_bundle_executable_when_it_differs_from_app_name(self):
        # Production mutation caught: deriving the executable name from Fixture.app
        # instead of reading the authoritative CFBundleExecutable value.
        with tempfile.TemporaryDirectory() as directory:
            app, dmg, _ = create_release_fixture(Path(directory))
            sign_and_create_dmg(app, dmg, executable_name="hongguo-desktop")

            completed = run_verifier(app, dmg)

        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        self.assertIn("Fixture.app/Contents/MacOS/hongguo-desktop", completed.stdout)

    def test_missing_required_resources_fail_with_specific_messages(self):
        cases = (
            ("hongguo-api-aarch64-apple-darwin", "release verify: missing hongguo-api"),
            ("ffprobe-aarch64-apple-darwin", "release verify: missing ffprobe"),
            ("THIRD_PARTY_NOTICES.md", "release verify: missing THIRD_PARTY_NOTICES.md"),
        )
        for filename, expected in cases:
            with self.subTest(filename=filename), tempfile.TemporaryDirectory() as directory:
                app, dmg, _ = create_release_fixture(Path(directory))
                (app / "Contents" / "Resources" / filename).unlink()

                completed = run_verifier(app, dmg)

                self.assertNotEqual(completed.returncode, 0)
                self.assertIn(expected, completed.stdout + completed.stderr)

    def test_non_executable_and_wrong_arch_tools_are_rejected_before_probes(self):
        with tempfile.TemporaryDirectory() as directory:
            app, dmg, _ = create_release_fixture(Path(directory))
            ffprobe = app / "Contents" / "Resources" / "ffprobe-aarch64-apple-darwin"
            ffprobe.chmod(0o644)

            completed = run_verifier(app, dmg)

            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("release verify: non-executable ffprobe", completed.stdout + completed.stderr)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app, dmg, _ = create_release_fixture(root)
            x86_source = root / "x86.c"
            x86_source.write_text("int main(void) { return 0; }\n", encoding="utf-8")
            ffmpeg = app / "Contents" / "Resources" / "ffmpeg-aarch64-apple-darwin"
            subprocess.run(
                ["/usr/bin/clang", "-arch", "x86_64", str(x86_source), "-o", str(ffmpeg)],
                check=True,
                capture_output=True,
                text=True,
            )

            completed = run_verifier(app, dmg)

            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("is not Mach-O ARM64", completed.stdout + completed.stderr)

    def test_credentials_tokens_and_known_secret_values_are_rejected(self):
        cases = (
            ("oauth-client.json", b"{}"),
            ("refresh-token-cache", b"opaque"),
            ("harmless-name.txt", b"Bearer secret-value"),
        )
        for filename, payload in cases:
            with self.subTest(filename=filename), tempfile.TemporaryDirectory() as directory:
                app, dmg, _ = create_release_fixture(Path(directory))
                (app / "Contents" / "Resources" / filename).write_bytes(payload)

                completed = run_verifier(app, dmg)

                self.assertNotEqual(completed.returncode, 0)
                output = completed.stdout + completed.stderr
                self.assertTrue(
                    "credential, token, or AI model payload" in output
                    or "synthetic secret fixture" in output,
                    output,
                )

    def test_sidecar_health_probe_failure_is_not_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app, dmg, _ = create_release_fixture(root)
            source = root / "failed-sidecar.c"
            source.write_text("int main(void) { return 7; }\n", encoding="utf-8")
            sidecar = app / "Contents" / "Resources" / "hongguo-api-aarch64-apple-darwin"
            subprocess.run(
                ["/usr/bin/clang", str(source), "-o", str(sidecar)],
                check=True,
                capture_output=True,
                text=True,
            )

            completed = run_verifier(app, dmg)

            self.assertNotEqual(completed.returncode, 0)

    def test_rejects_manifest_entrypoints_that_do_not_match_runtime_layout(self):
        # Production mutation caught: accepting model.bin placeholders that install
        # successfully but cannot be consumed by the packaged AI workers.
        with tempfile.TemporaryDirectory() as directory:
            app, dmg, manifest_path = create_release_fixture(Path(directory))
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["components"][1]["entrypoint"] = "model.bin"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

            completed = run_verifier(app, dmg)

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn(
            "release verify: invalid AI component entrypoint",
            completed.stdout + completed.stderr,
        )

    def test_rejects_demucs_weight_payload_in_the_base_app(self):
        # Production mutation caught: shipping optional multi-gigabyte model weights
        # inside the base app instead of installing verified components on demand.
        with tempfile.TemporaryDirectory() as directory:
            app, dmg, _ = create_release_fixture(Path(directory))
            (app / "Contents" / "Resources" / "955717e8-deadbeef.th").write_bytes(
                b"weights"
            )

            completed = run_verifier(app, dmg)

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn(
            "release verify: credential, token, or AI model payload found in base bundle",
            completed.stdout + completed.stderr,
        )


if __name__ == "__main__":
    unittest.main()
