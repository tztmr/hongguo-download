#!/bin/bash
set -euo pipefail

if [[ "$#" != 2 ]]; then
  echo "usage: verify-release.sh <app-path> <dmg-path>" >&2
  exit 2
fi

app_path="$1"
dmg_path="$2"
if [[ ! -d "$app_path" || "${app_path##*.}" != "app" ]]; then
  echo "release verify: app bundle is missing" >&2
  exit 1
fi
if [[ ! -f "$dmg_path" || "${dmg_path##*.}" != "dmg" ]]; then
  echo "release verify: dmg artifact is missing" >&2
  exit 1
fi

find_one() {
  local base="$1"
  local found
  found="$(find "$app_path/Contents" -type f \( -name "$base" -o -name "$base-*" \) -print -quit)"
  if [[ -z "$found" ]]; then
    echo "release verify: missing $base" >&2
    exit 1
  fi
  printf '%s\n' "$found"
}

verify_arm64_executable() {
  local path="$1"
  if [[ ! -x "$path" ]]; then
    echo "release verify: non-executable $(basename "$path")" >&2
    exit 1
  fi
  if [[ "$(/usr/bin/file -b "$path")" != *"Mach-O"* ]] || ! /usr/bin/lipo "$path" -verify_arch arm64 >/dev/null 2>&1; then
    echo "release verify: $(basename "$path") is not Mach-O ARM64" >&2
    exit 1
  fi
}

api_sidecar="$(find_one 'hongguo-api')"
ffmpeg="$(find_one 'ffmpeg')"
ffprobe="$(find_one 'ffprobe')"
notice="$(find_one 'THIRD_PARTY_NOTICES.md')"
manifest="$(find_one 'ai-components.json')"
script_dir="$(cd "$(dirname "$0")" && pwd)"
python3 "$script_dir/release_urls.py" --manifest "$manifest"
verify_arm64_executable "$api_sidecar"
verify_arm64_executable "$ffmpeg"
verify_arm64_executable "$ffprobe"
[[ -s "$notice" ]] || { echo "release verify: third-party notice is empty" >&2; exit 1; }

minimal_path="/usr/bin:/bin:/usr/sbin:/sbin"
probe_dir="$(mktemp -d "${TMPDIR:-/tmp}/hongguo-release-probe.XXXXXX")"
trap '/bin/rm -rf "$probe_dir"' EXIT
PATH="$minimal_path" HONGGUO_DATA_DIR="$probe_dir/data" "$api_sidecar" --health-probe >/dev/null
PATH="$minimal_path" "$ffmpeg" -version >/dev/null
PATH="$minimal_path" "$ffprobe" -version >/dev/null

python3 - "$manifest" <<'PY'
import json, re, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
items = data.get("components")
if not isinstance(items, list) or not items:
    raise SystemExit("release verify: invalid AI component manifest")
expected_entrypoints = {
    "runtime": "hongguo-ai-worker/hongguo-ai-worker",
    "demucs-htdemucs": "htdemucs.yaml",
    "demucs-htdemucs_ft": "htdemucs_ft.yaml",
    "whisper-small": "small.pt",
    "whisper-medium": "medium.pt",
}
if len(items) != len(expected_entrypoints) or {item.get("id") for item in items} != set(expected_entrypoints):
    raise SystemExit("release verify: incomplete AI component manifest")
for item in items:
    if not re.fullmatch(r"[0-9a-f]{64}", str(item.get("sha256", ""))):
        raise SystemExit("release verify: invalid AI component checksum")
    if item.get("entrypoint") != expected_entrypoints[item["id"]]:
        raise SystemExit("release verify: invalid AI component entrypoint")
PY

if find "$app_path/Contents" -type f \( -iname '*oauth*.json' -o -iname '*client_secret*.json' -o -iname '*refresh*token*' -o -iname 'model.bin' -o -iname '*.th' -o -iname '*.pt' -o -iname '*.pth' -o -iname '*.ckpt' -o -iname '*.safetensors' \) -print -quit | grep -q .; then
  echo "release verify: credential, token, or AI model payload found in base bundle" >&2
  exit 1
fi
if grep -R -a -l -E 'Bearer secret-value|synthetic-refresh-token-fixture' "$app_path/Contents" >/dev/null 2>&1; then
  echo "release verify: synthetic secret fixture found in bundle" >&2
  exit 1
fi

/usr/bin/codesign --verify --deep --strict "$app_path"
/usr/bin/hdiutil verify "$dmg_path" >/dev/null
# Valid sealed files and direct process startup do not prove that Gatekeeper
# will allow a browser-downloaded copy to open from Finder.
if /usr/sbin/spctl --assess --type execute "$app_path" >/dev/null 2>&1; then
  echo "release verify: Gatekeeper accepted this app"
else
  echo "release verify: Gatekeeper did not accept this app; first-open approval is required (see docs/MACOS_INSTALL.md)" >&2
fi
info_plist="$app_path/Contents/Info.plist"
if [[ ! -f "$info_plist" || -L "$info_plist" ]]; then
  echo "release verify: missing Info.plist" >&2
  exit 1
fi
main_executable="$(/usr/bin/plutil -extract CFBundleExecutable raw -o - "$info_plist" 2>/dev/null)" || {
  echo "release verify: invalid CFBundleExecutable" >&2
  exit 1
}
if [[ -z "$main_executable" || "$main_executable" == */* || "$main_executable" == *$'\n'* ]]; then
  echo "release verify: invalid CFBundleExecutable" >&2
  exit 1
fi
main_path="$app_path/Contents/MacOS/$main_executable"
if [[ ! -f "$main_path" || -L "$main_path" || ! -x "$main_path" ]]; then
  echo "release verify: missing main executable" >&2
  exit 1
fi
/usr/bin/shasum -a 256 "$main_path" "$dmg_path"
