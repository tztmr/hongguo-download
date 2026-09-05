#!/bin/bash
set -euo pipefail

: "${HONGGUO_AI_RUNTIME_ARCHIVE:?required}"

project_root="$(cd "$(dirname "$0")/.." && pwd)"
output_dir="${HONGGUO_AI_MODEL_SOURCE_DIR:-$project_root/dist/ai-model-sources}"
curl_bin="${HONGGUO_CURL_BIN:-/usr/bin/curl}"
temporary_dir="$(mktemp -d "${TMPDIR:-/tmp}/hongguo-ai-model-fetch.XXXXXX")"
trap '/bin/rm -rf "$temporary_dir"' EXIT

if [[ ! -f "$HONGGUO_AI_RUNTIME_ARCHIVE" || -L "$HONGGUO_AI_RUNTIME_ARCHIVE" ]]; then
  echo "AI runtime archive must be a regular non-link file" >&2
  exit 1
fi
if [[ ! -x "$curl_bin" ]]; then
  echo "curl executable is unavailable" >&2
  exit 1
fi

remote_root="hongguo-ai-worker/_internal/demucs/remote"
/usr/bin/tar -xzf "$HONGGUO_AI_RUNTIME_ARCHIVE" -C "$temporary_dir" \
  "$remote_root/htdemucs.yaml" \
  "$remote_root/htdemucs_ft.yaml"

mkdir -p "$output_dir"

download_model() {
  local url="$1"
  local expected="$2"
  local name="$3"
  local destination="$output_dir/$name"
  local partial="$destination.part"
  local actual

  if [[ -f "$destination" && ! -L "$destination" ]]; then
    actual="$(/usr/bin/shasum -a 256 "$destination" | /usr/bin/awk '{print $1}')"
    if [[ "${actual:0:${#expected}}" == "$expected" ]]; then
      return
    fi
  fi

  /bin/rm -f "$partial"
  "$curl_bin" --fail --location --retry 4 --retry-all-errors \
    "$url" --output "$partial"
  if [[ ! -f "$partial" || -L "$partial" ]]; then
    echo "model download did not produce a regular file: $name" >&2
    exit 1
  fi
  actual="$(/usr/bin/shasum -a 256 "$partial" | /usr/bin/awk '{print $1}')"
  if [[ "${actual:0:${#expected}}" != "$expected" ]]; then
    /bin/rm -f "$partial"
    echo "model SHA-256 mismatch: $name" >&2
    exit 1
  fi
  /bin/chmod 644 "$partial"
  /bin/mv "$partial" "$destination"
}

demucs_root="https://dl.fbaipublicfiles.com/demucs/hybrid_transformer"
download_model "$demucs_root/955717e8-8726e21a.th" "8726e21a" "955717e8-8726e21a.th"
download_model "$demucs_root/f7e0c4bc-ba3fe64a.th" "ba3fe64a" "f7e0c4bc-ba3fe64a.th"
download_model "$demucs_root/d12395a8-e57c48e6.th" "e57c48e6" "d12395a8-e57c48e6.th"
download_model "$demucs_root/92cfc3b6-ef3bcb9c.th" "ef3bcb9c" "92cfc3b6-ef3bcb9c.th"
download_model "$demucs_root/04573f0d-f3cf25b2.th" "f3cf25b2" "04573f0d-f3cf25b2.th"

whisper_root="https://openaipublic.azureedge.net/main/whisper/models"
download_model \
  "$whisper_root/9ecf779972d90ba49c06d968637d720dd632c55bbf19d441fb42bf17a411e794/small.pt" \
  "9ecf779972d90ba49c06d968637d720dd632c55bbf19d441fb42bf17a411e794" \
  "small.pt"
download_model \
  "$whisper_root/345ae4da62f9b3d59415adc60127b97c714f32e89e936602e85993674d08dcb1/medium.pt" \
  "345ae4da62f9b3d59415adc60127b97c714f32e89e936602e85993674d08dcb1" \
  "medium.pt"

/usr/bin/install -m 644 "$temporary_dir/$remote_root/htdemucs.yaml" "$output_dir/htdemucs.yaml"
/usr/bin/install -m 644 "$temporary_dir/$remote_root/htdemucs_ft.yaml" "$output_dir/htdemucs_ft.yaml"
printf '%s\n' "$output_dir"
