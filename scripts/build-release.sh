#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"

required_env=(
  HONGGUO_FFMPEG_ARCHIVE HONGGUO_FFMPEG_SHA256 HONGGUO_FFPROBE_SHA256
  HONGGUO_AI_RUNTIME_URL HONGGUO_AI_RUNTIME_SHA256 HONGGUO_AI_RUNTIME_DOWNLOAD_BYTES HONGGUO_AI_RUNTIME_INSTALLED_BYTES HONGGUO_AI_RUNTIME_VERSION
  HONGGUO_AI_DEMUCS_URL HONGGUO_AI_DEMUCS_SHA256 HONGGUO_AI_DEMUCS_DOWNLOAD_BYTES HONGGUO_AI_DEMUCS_INSTALLED_BYTES HONGGUO_AI_DEMUCS_VERSION
  HONGGUO_AI_DEMUCS_FT_URL HONGGUO_AI_DEMUCS_FT_SHA256 HONGGUO_AI_DEMUCS_FT_DOWNLOAD_BYTES HONGGUO_AI_DEMUCS_FT_INSTALLED_BYTES HONGGUO_AI_DEMUCS_FT_VERSION
  HONGGUO_AI_WHISPER_URL HONGGUO_AI_WHISPER_SHA256 HONGGUO_AI_WHISPER_DOWNLOAD_BYTES HONGGUO_AI_WHISPER_INSTALLED_BYTES HONGGUO_AI_WHISPER_VERSION
  HONGGUO_AI_WHISPER_MEDIUM_URL HONGGUO_AI_WHISPER_MEDIUM_SHA256 HONGGUO_AI_WHISPER_MEDIUM_DOWNLOAD_BYTES HONGGUO_AI_WHISPER_MEDIUM_INSTALLED_BYTES HONGGUO_AI_WHISPER_MEDIUM_VERSION
)

for name in "${required_env[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    echo "release preflight: missing required variable $name" >&2
    exit 1
  fi
done
if [[ ! -f "$HONGGUO_FFMPEG_ARCHIVE" ]]; then
  echo "release preflight: media tool archive does not exist" >&2
  exit 1
fi

python3 "$project_root/scripts/release_urls.py" --env

cd "$project_root"
./scripts/build-api-sidecar.sh
./scripts/stage-media-tools.sh
./scripts/render-ai-component-manifest.sh
python3 -m unittest discover -s tests -v
python3 -m unittest discover -s ai_worker/tests -v
(cd desktop && npm test && npm run build)
cargo fmt --manifest-path desktop/src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path desktop/src-tauri/Cargo.toml
(cd desktop && npm run tauri -- build --config src-tauri/tauri.release.conf.json)

tauri_config="$project_root/desktop/src-tauri/tauri.conf.json"
product_name="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["productName"])' "$tauri_config")"
product_version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])' "$tauri_config")"
artifact_paths="$(
  "$project_root/scripts/resolve-release-artifacts.sh" \
    "$project_root/desktop/src-tauri/target/release/bundle" \
    "$product_name" \
    "$product_version" \
    "aarch64"
)"
app_path="${artifact_paths%%$'\n'*}"
dmg_path="${artifact_paths#*$'\n'}"
./scripts/verify-release.sh "$app_path" "$dmg_path"

python3 "$project_root/scripts/verify-app-startup.py" "$app_path/Contents/MacOS/hongguo-desktop" "$product_version"
