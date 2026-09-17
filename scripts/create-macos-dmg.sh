#!/bin/bash
set -euo pipefail

if [[ "$#" != 2 ]]; then
  echo "usage: create-macos-dmg.sh <app-path> <dmg-path>" >&2
  exit 2
fi
app_path="$1"
dmg_path="$2"
script_dir="$(cd "$(dirname "$0")" && pwd)"
[[ -d "$app_path" && "$app_path" == *.app && "$dmg_path" == *.dmg ]] || {
  echo "macOS packaging: expected an app bundle and a DMG output path" >&2
  exit 1
}
/usr/bin/codesign --verify --deep --strict "$app_path"
version="$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$app_path/Contents/Info.plist")"
stage="$(mktemp -d "${TMPDIR:-/tmp}/hongguo-dmg.XXXXXX")"
trap '/bin/rm -rf "$stage"' EXIT
mkdir -p "$stage/content" "$(dirname "$dmg_path")"
/usr/bin/ditto "$app_path" "$stage/content/$(basename "$app_path")"
/bin/ln -s /Applications "$stage/content/Applications"
/bin/cp "$script_dir/../docs/MACOS_INSTALL.md" "$stage/content/安装与首次打开.txt"
# Creating a plain image avoids Finder/AppleScript layout automation during
# unattended builds. The app and install guidance are both visible at its root.
/usr/bin/hdiutil create -ov -volname "红果下载 $version" -fs HFS+ -format UDZO \
  -srcfolder "$stage/content" "$dmg_path"
/usr/bin/hdiutil verify "$dmg_path" >/dev/null
