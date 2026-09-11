"""Record the API inputs compiled into a standalone sidecar."""
import hashlib
import json
import sys
from pathlib import Path


def source_manifest(root: Path) -> dict:
    paths = [root / name for name in ("main.py", "requirements.txt", "requirements-build.txt")]
    for folder in ("core", "endpoints"):
        paths.extend((root / folder).rglob("*.py"))
    return {"schema": 1, "source_files": {
        path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(paths)
    }}


if __name__ == "__main__":
    Path(sys.argv[1]).write_text(
        json.dumps(source_manifest(Path(__file__).resolve().parents[1]), sort_keys=True),
        encoding="utf-8",
    )
