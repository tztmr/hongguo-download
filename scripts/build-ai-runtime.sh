#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
destination_dir="${HONGGUO_AI_OUTPUT_DIR:-$project_root/dist/ai-components}"
temporary_dir="$(mktemp -d "${TMPDIR:-/tmp}/hongguo-ai-build.XXXXXX")"
trap 'rm -rf "$temporary_dir"' EXIT

if [[ "$(/usr/bin/uname -s)" != "Darwin" ]] || [[ "$(/usr/bin/uname -m)" != "arm64" ]]; then
  echo "AI runtime must be built on macOS ARM64" >&2
  exit 1
fi

python_bin="${PYTHON_BIN:-python3}"
"$python_bin" -m venv "$temporary_dir/venv"
venv_python="$temporary_dir/venv/bin/python"
"$venv_python" -m pip install --disable-pip-version-check -r "$project_root/requirements-ai.txt"
"$venv_python" -m PyInstaller \
  --clean \
  --noconfirm \
  --onedir \
  --name hongguo-ai-worker \
  --paths "$project_root" \
  --collect-all torch \
  --collect-all demucs \
  --collect-all whisper \
  --distpath "$temporary_dir/dist" \
  --workpath "$temporary_dir/work" \
  --specpath "$temporary_dir/spec" \
  "$project_root/ai_worker/main.py"

worker="$temporary_dir/dist/hongguo-ai-worker/hongguo-ai-worker"
if [[ ! -x "$worker" ]]; then
  echo "PyInstaller did not produce the AI worker" >&2
  exit 1
fi
"$worker" --self-test >/dev/null

mkdir -p "$destination_dir"
archive="$destination_dir/hongguo-ai-runtime-aarch64-apple-darwin.tar.gz"
/usr/bin/tar -czf "$archive" -C "$temporary_dir/dist" hongguo-ai-worker
/usr/bin/shasum -a 256 "$archive"
