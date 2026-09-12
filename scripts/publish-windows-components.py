"""Publish successful CI artifacts without transferring multi-GB runtimes locally."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def byte_count(value):
    # PowerShell Measure-Object serializes integral totals as 123.0. The
    # desktop manifest deserializes u64 and requires JSON integers.
    assert type(value) in (int, float) and 0 < value < 2**53
    assert int(value) == value, "Byte count must be integral"
    return int(value)


def main():
    repo, run_id, tag = (os.environ[name] for name in ("GH_REPO", "SOURCE_RUN_ID", "COMPONENT_TAG"))
    release_repo = os.environ.get("COMPONENT_RELEASE_REPO", "tztmr/apihongguoai-address")
    assert re.fullmatch(r"[0-9]+", run_id)
    assert re.fullmatch(r"windows-components-v[0-9]+", tag)
    version = tag.removeprefix("windows-components-v")

    def api(path, repository=repo):
        endpoint = f"repos/{repository}" + (f"/{path.lstrip('/')}" if path else "")
        return json.loads(subprocess.check_output(["gh", "api", endpoint]))

    run = api(f"actions/runs/{run_id}")
    assert run["status"] == "completed" and run["conclusion"] == "success", "Runtime CI must pass first"
    assert run["path"] == ".github/workflows/windows-components.yml", "Unexpected build workflow"
    assert run["head_repository"]["full_name"] == repo, "Unexpected source repository"
    revision = run["head_sha"]
    destination = api("", release_repo)
    assert destination.get("visibility") == "public" and destination.get("private") is False, "Component downloads require a public release repository"
    target = api(f"commits/{destination['default_branch']}", release_repo)["sha"]
    provenance = f"Source: {repo}@{revision}. Build: {run['html_url']}"
    source = api(f"contents/desktop/src-tauri/resources/ai-components.windows.json?ref={revision}")
    manifest = json.loads(base64.b64decode(source["content"]))
    pages = json.loads(subprocess.check_output(["gh", "api", "--paginate", "--slurp", f"repos/{release_repo}/releases?per_page=100"]))
    matches = [release for page in pages for release in page if release["tag_name"] == tag]
    if not matches:
        subprocess.run(["gh", "release", "create", tag, "--repo", release_repo, "--draft", "--target", target,
                        "--title", f"Windows AI runtimes v{version}", "--notes",
                        f"Verified Windows modern, legacy and CPU runtimes. {provenance}"], check=True)
        pages = json.loads(subprocess.check_output(["gh", "api", "--paginate", "--slurp", f"repos/{release_repo}/releases?per_page=100"]))
        matches = [release for page in pages for release in page if release["tag_name"] == tag]
    assert len(matches) == 1
    assert matches[0]["draft"] and provenance in (matches[0].get("body") or ""), "Only a matching draft can be updated"
    release_id = matches[0]["id"]

    def upload(path):
        release = api(f"releases/{release_id}", release_repo)
        assert release["draft"] and provenance in (release.get("body") or ""), "Release changed during publication"
        existing = [asset for asset in release["assets"] if asset["name"] == path.name]
        if existing:
            assert len(existing) == 1 and existing[0]["state"] == "uploaded"
            assert existing[0]["size"] == path.stat().st_size and existing[0]["digest"] == "sha256:" + digest(path), "Existing asset differs"
        else:
            subprocess.run(["gh", "release", "upload", tag, str(path), "--repo", release_repo], check=True)

    for flavor in ("cpu", "legacy", "modern"):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stem = f"hongguo-ai-runtime-windows-{flavor}"
            subprocess.run(["gh", "run", "download", run_id, "--repo", repo, "--name", stem, "--dir", str(root)], check=True)
            archive = root / f"{stem}.zip"
            metadata = json.loads((root / f"{stem}.json").read_text(encoding="utf-8-sig"))
            assert metadata["Flavor"] == flavor and archive.is_file()
            assert digest(archive) == metadata["Sha256"] and archive.stat().st_size == metadata["DownloadBytes"], "Archive checksum mismatch"
            metadata["DownloadBytes"] = byte_count(metadata["DownloadBytes"])
            metadata["InstalledBytes"] = byte_count(metadata["InstalledBytes"])
            url = f"https://github.com/{release_repo}/releases/download/{tag}/{archive.name}"
            item = next(item for item in manifest["components"] if item["id"] == f"runtime-{flavor}")
            item.update(version=version, url=url, sha256=metadata["Sha256"], downloadBytes=metadata["DownloadBytes"], installedBytes=metadata["InstalledBytes"])
            item.pop("parts", None)
            if archive.stat().st_size > 2_000_000_000:
                parts = []
                with archive.open("rb") as stream:
                    index = 1
                    while stream.tell() < archive.stat().st_size:
                        part = root / f"{archive.name}.{index:03d}"
                        with part.open("wb") as target:
                            remaining = 2_000_000_000
                            while remaining and (block := stream.read(min(1024 * 1024, remaining))):
                                target.write(block)
                                remaining -= len(block)
                        upload(part)
                        parts.append({"url": url + f".{index:03d}", "bytes": part.stat().st_size})
                        part.unlink()
                        index += 1
                item["parts"] = parts
            else:
                upload(archive)
            print(f"Verified and uploaded {flavor}", flush=True)
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / "ai-components.windows.json"
        path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        upload(path)
    subprocess.run(["gh", "release", "edit", tag, "--repo", release_repo, "--draft=false", "--latest=false"], check=True)


if __name__ == "__main__":
    main()
