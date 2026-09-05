#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
destination_dir="$project_root/desktop/src-tauri/binaries"
temporary_dir="$(mktemp -d "${TMPDIR:-/tmp}/hongguo-api-build.XXXXXX")"
trap 'rm -rf "$temporary_dir"' EXIT

if [[ "$(/usr/bin/uname -s)" != "Darwin" ]] || [[ "$(/usr/bin/uname -m)" != "arm64" ]]; then
  echo "hongguo-api sidecar must be built on macOS ARM64" >&2
  exit 1
fi

python_bin="${PYTHON_BIN:-python3}"
"$python_bin" -m venv "$temporary_dir/venv"
venv_python="$temporary_dir/venv/bin/python"
"$venv_python" -m pip install --disable-pip-version-check \
  -r "$project_root/requirements.txt" \
  -r "$project_root/requirements-build.txt"

"$venv_python" -m PyInstaller \
  --clean \
  --noconfirm \
  --onefile \
  --name hongguo-api \
  --distpath "$temporary_dir/dist" \
  --workpath "$temporary_dir/work" \
  --specpath "$temporary_dir/spec" \
  "$project_root/main.py"

built_sidecar="$temporary_dir/dist/hongguo-api"
if [[ ! -x "$built_sidecar" ]]; then
  echo "PyInstaller did not produce an executable hongguo-api" >&2
  exit 1
fi
if [[ "$(/usr/bin/file -b "$built_sidecar")" != *"Mach-O"* ]] || \
  ! /usr/bin/lipo "$built_sidecar" -verify_arch arm64 >/dev/null 2>&1; then
  echo "hongguo-api is not a Mach-O ARM64 executable" >&2
  exit 1
fi

probe_data_dir="$temporary_dir/probe-data"
mkdir -p "$probe_data_dir"
HONGGUO_DATA_DIR="$probe_data_dir" "$built_sidecar" --health-probe

mkdir -p "$destination_dir"
staged="$destination_dir/.hongguo-api-aarch64-apple-darwin.$$"
/usr/bin/install -m 755 "$built_sidecar" "$staged"
/bin/mv "$staged" "$destination_dir/hongguo-api-aarch64-apple-darwin"
