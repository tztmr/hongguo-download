import importlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from hashlib import sha256
from pathlib import Path
from unittest.mock import patch


PROJECT_ROOT = Path(__file__).resolve().parents[1]
STAGING_SCRIPT = PROJECT_ROOT / "scripts" / "stage-media-tools.sh"
TAURI_DIR = PROJECT_ROOT / "desktop" / "src-tauri"
RELEASE_CONFIG = TAURI_DIR / "tauri.release.conf.json"

TOOL_SOURCE = r"""
#include <stdio.h>
#include <string.h>
#ifdef WITH_FIXTURE_DEPENDENCY
extern int fixture_dependency(void);
#endif

int main(int argc, char **argv) {
#ifdef WITH_FIXTURE_DEPENDENCY
    if (fixture_dependency() != 0) return 9;
#endif
    for (int index = 1; index < argc; index++) {
        if (strcmp(argv[index], "-encoders") == 0) {
            puts(" V..... h264_videotoolbox fixture encoder");
            puts(" V..... libx264 fixture encoder");
            return 0;
        }
    }
    puts("fixture media tool version 1");
    return 0;
}
"""

DEPENDENCY_SOURCE = "int fixture_dependency(void) { return 0; }\n"


def create_tool_archive(root: Path) -> tuple[Path, str, str]:
    tools_dir = root / "archive-input"
    tools_dir.mkdir()
    payloads = {
        "ffmpeg": b"#!/bin/sh\necho ffmpeg fixture\n",
        "ffprobe": b"#!/bin/sh\necho ffprobe fixture\n",
    }
    for name, payload in payloads.items():
        path = tools_dir / name
        path.write_bytes(payload)
        path.chmod(0o755)
    archive = root / "media-tools.tar.gz"
    with tarfile.open(archive, "w:gz") as bundle:
        for name in payloads:
            bundle.add(tools_dir / name, arcname=name)
    return archive, sha256(payloads["ffmpeg"]).hexdigest(), sha256(payloads["ffprobe"]).hexdigest()


def compile_media_tool_archive(
    root: Path,
    *,
    unsafe_homebrew_rpath: bool = False,
    disguised_system_rpath_escape: bool = False,
) -> tuple[Path, str, str, Path, Path]:
    build_dir = root / "compiled-tools"
    build_dir.mkdir()
    source = build_dir / "tool.c"
    source.write_text(TOOL_SOURCE)
    ffmpeg = build_dir / "ffmpeg"
    command = ["/usr/bin/clang", str(source), "-o", str(ffmpeg)]
    if unsafe_homebrew_rpath or disguised_system_rpath_escape:
        dependency_source = build_dir / "dependency.c"
        dependency_source.write_text(DEPENDENCY_SOURCE)
        dependency = build_dir / "libfixture.dylib"
        subprocess.run(
            [
                "/usr/bin/clang",
                "-dynamiclib",
                str(dependency_source),
                "-Wl,-install_name,@rpath/libfixture.dylib",
                "-o",
                str(dependency),
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        command.extend(["-DWITH_FIXTURE_DEPENDENCY", str(dependency)])
        if unsafe_homebrew_rpath:
            command.extend(
                ["-Wl,-rpath,/opt/homebrew/lib", f"-Wl,-rpath,{build_dir}"]
            )
        if disguised_system_rpath_escape:
            canonical_build_dir = build_dir.resolve()
            command.append(f"-Wl,-rpath,/usr/lib/../..{canonical_build_dir}")
    subprocess.run(command, check=True, capture_output=True, text=True)
    ffprobe = build_dir / "ffprobe"
    shutil.copy2(ffmpeg, ffprobe)
    archive = root / "compiled-media-tools.tar.gz"
    with tarfile.open(archive, "w:gz") as bundle:
        bundle.add(ffmpeg, arcname="ffmpeg")
        bundle.add(ffprobe, arcname="ffprobe")
    return (
        archive,
        sha256(ffmpeg.read_bytes()).hexdigest(),
        sha256(ffprobe.read_bytes()).hexdigest(),
        ffmpeg,
        ffprobe,
    )


def create_staging_sandbox(root: Path) -> tuple[Path, Path]:
    sandbox = root / "staging-sandbox"
    scripts_dir = sandbox / "scripts"
    destination = sandbox / "desktop" / "src-tauri" / "binaries"
    scripts_dir.mkdir(parents=True)
    destination.mkdir(parents=True)
    script = scripts_dir / STAGING_SCRIPT.name
    shutil.copy2(STAGING_SCRIPT, script)
    return script, destination


def staging_environment(archive: Path, ffmpeg_sha: str, ffprobe_sha: str) -> dict[str, str]:
    return {
        **os.environ,
        "HONGGUO_FFMPEG_ARCHIVE": str(archive),
        "HONGGUO_FFMPEG_SHA256": ffmpeg_sha,
        "HONGGUO_FFPROBE_SHA256": ffprobe_sha,
    }


def run_staging_script(
    env: dict[str, str], script: Path = STAGING_SCRIPT
) -> subprocess.CompletedProcess[str]:
    if not script.is_file():
        raise AssertionError(f"staging script is missing: {script}")
    return subprocess.run(
        ["/bin/bash", str(script)],
        cwd=script.parent.parent,
        env=env,
        text=True,
        capture_output=True,
        timeout=10,
        check=False,
    )


class DevicePoolDataDirTest(unittest.TestCase):
    def test_uses_external_data_directory_when_configured(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            with patch.dict(os.environ, {"HONGGUO_DATA_DIR": temp_dir}):
                sys.modules.pop("core.device_pool", None)
                module = importlib.import_module("core.device_pool")

            self.assertEqual(module.POOL_FILE, Path(temp_dir) / "device_pool.json")


class SidecarToolingTest(unittest.TestCase):
    def test_ad_hoc_bundle_disables_hardened_runtime_for_pyinstaller_sidecar(self):
        config = json.loads((TAURI_DIR / "tauri.conf.json").read_text())

        self.assertFalse(config["bundle"]["macOS"]["hardenedRuntime"])

    def test_health_probe_imports_app_and_exits_without_starting_server(self):
        # Production mutation caught: routing --health-probe through uvicorn.run.
        with tempfile.TemporaryDirectory() as temp_dir:
            process = subprocess.Popen(
                [sys.executable, "-B", "main.py", "--health-probe"],
                cwd=PROJECT_ROOT,
                env={**os.environ, "HONGGUO_DATA_DIR": temp_dir},
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            try:
                stdout, stderr = process.communicate(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate()
                self.fail("--health-probe did not exit; it likely started the listening server")

        self.assertEqual(process.returncode, 0, stderr)
        self.assertNotIn("Uvicorn running on", stdout + stderr)

    def test_staging_rejects_missing_required_checksum_environment(self):
        # Production mutation caught: defaulting or ignoring a required caller checksum.
        with tempfile.TemporaryDirectory() as temp_dir:
            archive, _, _ = create_tool_archive(Path(temp_dir))
            env = {
                **os.environ,
                "HONGGUO_FFMPEG_ARCHIVE": str(archive),
            }
            env.pop("HONGGUO_FFMPEG_SHA256", None)
            env.pop("HONGGUO_FFPROBE_SHA256", None)

            completed = run_staging_script(env)

        self.assertNotEqual(completed.returncode, 0)

    def test_staging_rejects_wrong_sha256(self):
        # Production mutation caught: staging archive members without verifying their digests.
        with tempfile.TemporaryDirectory() as temp_dir:
            archive, _, ffprobe_sha = create_tool_archive(Path(temp_dir))
            completed = run_staging_script(
                {
                    **os.environ,
                    "HONGGUO_FFMPEG_ARCHIVE": str(archive),
                    "HONGGUO_FFMPEG_SHA256": "0" * 64,
                    "HONGGUO_FFPROBE_SHA256": ffprobe_sha,
                }
            )

        self.assertNotEqual(completed.returncode, 0)

    def test_staging_rejects_non_self_contained_homebrew_rpath(self):
        # Production mutation caught: trusting matching hashes while accepting unsafe LC_RPATH entries.
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            archive, ffmpeg_sha, ffprobe_sha, _, _ = compile_media_tool_archive(
                root, unsafe_homebrew_rpath=True
            )
            script, destination = create_staging_sandbox(root)

            completed = run_staging_script(
                staging_environment(archive, ffmpeg_sha, ffprobe_sha), script
            )

            self.assertNotEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            self.assertIn(
                "unsafe LC_RPATH: /opt/homebrew/lib",
                completed.stdout + completed.stderr,
            )
            self.assertFalse((destination / "ffmpeg-aarch64-apple-darwin").exists())
            self.assertFalse((destination / "ffprobe-aarch64-apple-darwin").exists())

    def test_staging_rejects_canonical_escape_from_system_prefixed_rpath(self):
        # Production mutation caught: allowlisting an absolute rpath before normalizing its components.
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            archive, ffmpeg_sha, ffprobe_sha, _, _ = compile_media_tool_archive(
                root, disguised_system_rpath_escape=True
            )
            script, destination = create_staging_sandbox(root)

            completed = run_staging_script(
                staging_environment(archive, ffmpeg_sha, ffprobe_sha), script
            )

            self.assertNotEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            self.assertIn("unsafe LC_RPATH", completed.stdout + completed.stderr)
            self.assertFalse((destination / "ffmpeg-aarch64-apple-darwin").exists())
            self.assertFalse((destination / "ffprobe-aarch64-apple-darwin").exists())

    def test_staging_accepts_system_only_macho_fixture_as_a_pair(self):
        # Production mutation caught: making the dependency allowlist reject system-only Mach-O tools.
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            archive, ffmpeg_sha, ffprobe_sha, _, _ = compile_media_tool_archive(root)
            script, destination = create_staging_sandbox(root)

            completed = run_staging_script(
                staging_environment(archive, ffmpeg_sha, ffprobe_sha), script
            )

            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            self.assertEqual(
                sha256((destination / "ffmpeg-aarch64-apple-darwin").read_bytes()).hexdigest(),
                ffmpeg_sha,
            )
            self.assertEqual(
                sha256((destination / "ffprobe-aarch64-apple-darwin").read_bytes()).hexdigest(),
                ffprobe_sha,
            )

    def test_staging_rejects_tar_symlink_escape(self):
        # Production mutation caught: extracting and following a tar symlink to an outside executable.
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            _, _, ffprobe_sha, ffmpeg, ffprobe = compile_media_tool_archive(root)
            archive = root / "symlink-media-tools.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                link = tarfile.TarInfo("ffmpeg")
                link.type = tarfile.SYMTYPE
                link.linkname = str(ffmpeg)
                link.mode = 0o755
                bundle.addfile(link)
                bundle.add(ffprobe, arcname="ffprobe")
            script, destination = create_staging_sandbox(root)

            completed = run_staging_script(
                staging_environment(
                    archive, sha256(ffmpeg.read_bytes()).hexdigest(), ffprobe_sha
                ),
                script,
            )

            self.assertNotEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            self.assertFalse((destination / "ffmpeg-aarch64-apple-darwin").exists())

    def test_staging_rejects_zip_symlink_escape(self):
        # Production mutation caught: extracting and following a Unix symlink encoded in a zip member.
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            _, _, ffprobe_sha, ffmpeg, ffprobe = compile_media_tool_archive(root)
            archive = root / "symlink-media-tools.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                link = zipfile.ZipInfo("ffmpeg")
                link.create_system = 3
                link.external_attr = (stat.S_IFLNK | 0o755) << 16
                bundle.writestr(link, str(ffmpeg))
                bundle.write(ffprobe, arcname="ffprobe")
            script, destination = create_staging_sandbox(root)

            completed = run_staging_script(
                staging_environment(
                    archive, sha256(ffmpeg.read_bytes()).hexdigest(), ffprobe_sha
                ),
                script,
            )

            self.assertNotEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            self.assertFalse((destination / "ffmpeg-aarch64-apple-darwin").exists())

    def test_staging_rejects_tar_hardlink_member(self):
        # Production mutation caught: accepting a tar hardlink as one member of the release pair.
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            _, ffmpeg_sha, _, ffmpeg, _ = compile_media_tool_archive(root)
            archive = root / "hardlink-media-tools.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                bundle.add(ffmpeg, arcname="ffmpeg")
                link = tarfile.TarInfo("ffprobe")
                link.type = tarfile.LNKTYPE
                link.linkname = "ffmpeg"
                link.mode = 0o755
                bundle.addfile(link)
            script, destination = create_staging_sandbox(root)

            completed = run_staging_script(
                staging_environment(archive, ffmpeg_sha, ffmpeg_sha), script
            )

            self.assertNotEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            self.assertFalse((destination / "ffprobe-aarch64-apple-darwin").exists())

    def test_pair_commit_rolls_back_when_second_install_step_fails(self):
        # Production mutation caught: leaving new ffmpeg paired with old ffprobe after step two fails.
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            archive, ffmpeg_sha, ffprobe_sha, _, _ = compile_media_tool_archive(root)
            script, destination = create_staging_sandbox(root)
            installed_ffmpeg = destination / "ffmpeg-aarch64-apple-darwin"
            installed_ffprobe = destination / "ffprobe-aarch64-apple-darwin"
            installed_ffmpeg.write_bytes(b"old ffmpeg")
            installed_ffprobe.write_bytes(b"old ffprobe")
            env = staging_environment(archive, ffmpeg_sha, ffprobe_sha)
            env["HONGGUO_STAGE_FAIL_AFTER_FFMPEG_COMMIT"] = "1"

            completed = run_staging_script(env, script)

            self.assertNotEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            self.assertEqual(installed_ffmpeg.read_bytes(), b"old ffmpeg")
            self.assertEqual(installed_ffprobe.read_bytes(), b"old ffprobe")
            self.assertEqual(
                sorted(path.name for path in destination.iterdir()),
                ["ffmpeg-aarch64-apple-darwin", "ffprobe-aarch64-apple-darwin"],
            )

    def test_release_config_is_exact_and_missing_assets_fail_closed(self):
        # Production mutation caught: weakening release externalBin or silently omitting absent assets.
        if not RELEASE_CONFIG.is_file():
            self.fail(f"release Tauri config is missing: {RELEASE_CONFIG}")
        release_config = json.loads(RELEASE_CONFIG.read_text())
        base_config = json.loads((TAURI_DIR / "tauri.conf.json").read_text())
        external_bins = release_config.get("bundle", {}).get("externalBin")

        self.assertEqual(
            external_bins,
            [
                "binaries/hongguo-api",
                "binaries/ffmpeg",
                "binaries/ffprobe",
            ],
        )
        self.assertNotIn("externalBin", base_config.get("bundle", {}))

        missing_asset_config = json.loads(json.dumps(release_config))
        missing_asset_config["bundle"]["externalBin"] = [
            "binaries/hongguo-test-intentionally-missing"
        ]
        completed = subprocess.run(
            [
                "cargo",
                "check",
                "--manifest-path",
                str(TAURI_DIR / "Cargo.toml"),
            ],
            cwd=PROJECT_ROOT,
            env={**os.environ, "TAURI_CONFIG": json.dumps(missing_asset_config)},
            text=True,
            capture_output=True,
            timeout=30,
            check=False,
        )

        self.assertNotEqual(completed.returncode, 0)
        self.assertRegex(
            completed.stdout + completed.stderr,
            r"resource path `binaries/hongguo-test-intentionally-missing-aarch64-apple-darwin` doesn't exist",
        )


if __name__ == "__main__":
    unittest.main()
