#!/bin/bash
set -euo pipefail

: "${HONGGUO_AI_RUNTIME_URL:?required}"
: "${HONGGUO_AI_RUNTIME_SHA256:?required}"
: "${HONGGUO_AI_RUNTIME_DOWNLOAD_BYTES:?required}"
: "${HONGGUO_AI_RUNTIME_INSTALLED_BYTES:?required}"
: "${HONGGUO_AI_RUNTIME_VERSION:?required}"
: "${HONGGUO_AI_DEMUCS_URL:?required}"
: "${HONGGUO_AI_DEMUCS_SHA256:?required}"
: "${HONGGUO_AI_DEMUCS_DOWNLOAD_BYTES:?required}"
: "${HONGGUO_AI_DEMUCS_INSTALLED_BYTES:?required}"
: "${HONGGUO_AI_DEMUCS_VERSION:?required}"
: "${HONGGUO_AI_DEMUCS_FT_URL:?required}"
: "${HONGGUO_AI_DEMUCS_FT_SHA256:?required}"
: "${HONGGUO_AI_DEMUCS_FT_DOWNLOAD_BYTES:?required}"
: "${HONGGUO_AI_DEMUCS_FT_INSTALLED_BYTES:?required}"
: "${HONGGUO_AI_DEMUCS_FT_VERSION:?required}"
: "${HONGGUO_AI_WHISPER_URL:?required}"
: "${HONGGUO_AI_WHISPER_SHA256:?required}"
: "${HONGGUO_AI_WHISPER_DOWNLOAD_BYTES:?required}"
: "${HONGGUO_AI_WHISPER_INSTALLED_BYTES:?required}"
: "${HONGGUO_AI_WHISPER_VERSION:?required}"
: "${HONGGUO_AI_WHISPER_MEDIUM_URL:?required}"
: "${HONGGUO_AI_WHISPER_MEDIUM_SHA256:?required}"
: "${HONGGUO_AI_WHISPER_MEDIUM_DOWNLOAD_BYTES:?required}"
: "${HONGGUO_AI_WHISPER_MEDIUM_INSTALLED_BYTES:?required}"
: "${HONGGUO_AI_WHISPER_MEDIUM_VERSION:?required}"

validate_sha() {
  local value="$1"
  if [[ ! "$value" =~ ^[a-f0-9]{64}$ ]]; then
    echo "invalid sha256: $value" >&2
    exit 1
  fi
}

script_dir="$(cd "$(dirname "$0")" && pwd)"
python3 "$script_dir/release_urls.py" --env
validate_sha "$HONGGUO_AI_RUNTIME_SHA256"
validate_sha "$HONGGUO_AI_DEMUCS_SHA256"
validate_sha "$HONGGUO_AI_DEMUCS_FT_SHA256"
validate_sha "$HONGGUO_AI_WHISPER_SHA256"
validate_sha "$HONGGUO_AI_WHISPER_MEDIUM_SHA256"

output="${1:-desktop/src-tauri/resources/ai-components.json}"
mkdir -p "$(dirname "$output")"
cat > "$output" <<EOF
{
  "version": 1,
  "platform": "aarch64-apple-darwin",
  "components": [
    {
      "id": "runtime",
      "version": "$HONGGUO_AI_RUNTIME_VERSION",
      "platform": "aarch64-apple-darwin",
      "url": "$HONGGUO_AI_RUNTIME_URL",
      "sha256": "$HONGGUO_AI_RUNTIME_SHA256",
      "downloadBytes": $HONGGUO_AI_RUNTIME_DOWNLOAD_BYTES,
      "installedBytes": $HONGGUO_AI_RUNTIME_INSTALLED_BYTES,
      "entrypoint": "hongguo-ai-worker/hongguo-ai-worker"
    },
    {
      "id": "demucs-htdemucs",
      "version": "$HONGGUO_AI_DEMUCS_VERSION",
      "platform": "aarch64-apple-darwin",
      "url": "$HONGGUO_AI_DEMUCS_URL",
      "sha256": "$HONGGUO_AI_DEMUCS_SHA256",
      "downloadBytes": $HONGGUO_AI_DEMUCS_DOWNLOAD_BYTES,
      "installedBytes": $HONGGUO_AI_DEMUCS_INSTALLED_BYTES,
      "entrypoint": "htdemucs.yaml"
    },
    {
      "id": "whisper-small",
      "version": "$HONGGUO_AI_WHISPER_VERSION",
      "platform": "aarch64-apple-darwin",
      "url": "$HONGGUO_AI_WHISPER_URL",
      "sha256": "$HONGGUO_AI_WHISPER_SHA256",
      "downloadBytes": $HONGGUO_AI_WHISPER_DOWNLOAD_BYTES,
      "installedBytes": $HONGGUO_AI_WHISPER_INSTALLED_BYTES,
      "entrypoint": "small.pt"
    },
    {
      "id": "demucs-htdemucs_ft",
      "version": "$HONGGUO_AI_DEMUCS_FT_VERSION",
      "platform": "aarch64-apple-darwin",
      "url": "$HONGGUO_AI_DEMUCS_FT_URL",
      "sha256": "$HONGGUO_AI_DEMUCS_FT_SHA256",
      "downloadBytes": $HONGGUO_AI_DEMUCS_FT_DOWNLOAD_BYTES,
      "installedBytes": $HONGGUO_AI_DEMUCS_FT_INSTALLED_BYTES,
      "entrypoint": "htdemucs_ft.yaml"
    },
    {
      "id": "whisper-medium",
      "version": "$HONGGUO_AI_WHISPER_MEDIUM_VERSION",
      "platform": "aarch64-apple-darwin",
      "url": "$HONGGUO_AI_WHISPER_MEDIUM_URL",
      "sha256": "$HONGGUO_AI_WHISPER_MEDIUM_SHA256",
      "downloadBytes": $HONGGUO_AI_WHISPER_MEDIUM_DOWNLOAD_BYTES,
      "installedBytes": $HONGGUO_AI_WHISPER_MEDIUM_INSTALLED_BYTES,
      "entrypoint": "medium.pt"
    }
  ]
}
EOF
