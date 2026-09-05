#!/bin/bash
set -euo pipefail

: "${HONGGUO_FFMPEG_SOURCE_ARCHIVE:?required}"
: "${HONGGUO_FFMPEG_SOURCE_SHA256:?required}"
: "${HONGGUO_X264_PREFIX:?required}"

project_root="$(cd "$(dirname "$0")/.." && pwd)"
output_dir="${HONGGUO_MEDIA_TOOLS_OUTPUT:-$project_root/dist/media-tools}"
temporary_dir="$(mktemp -d "${TMPDIR:-/tmp}/hongguo-ffmpeg-build.XXXXXX")"
trap '/bin/rm -rf "$temporary_dir"' EXIT

if [[ "$(/usr/bin/uname -s)" != "Darwin" ]] || [[ "$(/usr/bin/uname -m)" != "arm64" ]]; then
  echo "media tools must be built on macOS ARM64" >&2
  exit 1
fi
if [[ ! "$HONGGUO_FFMPEG_SOURCE_SHA256" =~ ^[a-f0-9]{64}$ ]]; then
  echo "invalid FFmpeg source SHA-256" >&2
  exit 1
fi
if [[ -L "$HONGGUO_FFMPEG_SOURCE_ARCHIVE" || ! -f "$HONGGUO_FFMPEG_SOURCE_ARCHIVE" ]]; then
  echo "FFmpeg source archive must be a regular non-link file" >&2
  exit 1
fi
actual_source_sha="$(/usr/bin/shasum -a 256 "$HONGGUO_FFMPEG_SOURCE_ARCHIVE" | /usr/bin/awk '{print $1}')"
if [[ "$actual_source_sha" != "$HONGGUO_FFMPEG_SOURCE_SHA256" ]]; then
  echo "FFmpeg source SHA-256 mismatch" >&2
  exit 1
fi

x264_header="$HONGGUO_X264_PREFIX/include/x264.h"
x264_archive="$HONGGUO_X264_PREFIX/lib/libx264.a"
x264_pc="$HONGGUO_X264_PREFIX/lib/pkgconfig/x264.pc"
for path in "$x264_header" "$x264_archive" "$x264_pc"; do
  if [[ -L "$path" || ! -f "$path" ]]; then
    echo "x264 static build input is missing or unsafe: $(basename "$path")" >&2
    exit 1
  fi
done

source_parent="$temporary_dir/source"
mkdir -p "$source_parent"
/usr/bin/tar -xf "$HONGGUO_FFMPEG_SOURCE_ARCHIVE" -C "$source_parent"
source_dir="$(find "$source_parent" -mindepth 1 -maxdepth 1 -type d -name 'ffmpeg-*' -print -quit)"
if [[ -z "$source_dir" || ! -x "$source_dir/configure" ]]; then
  echo "FFmpeg source archive layout is invalid" >&2
  exit 1
fi

x264_isolated="$temporary_dir/x264"
mkdir -p "$x264_isolated/include" "$x264_isolated/lib/pkgconfig"
/bin/cp -R "$HONGGUO_X264_PREFIX/include/." "$x264_isolated/include/"
/bin/cp "$x264_archive" "$x264_isolated/lib/libx264.a"
/usr/bin/sed "s|^prefix=.*|prefix=$x264_isolated|" "$x264_pc" >"$x264_isolated/lib/pkgconfig/x264.pc"

install_dir="$temporary_dir/install"
build_dir="$temporary_dir/build"
mkdir -p "$build_dir"
jobs="$(/usr/sbin/sysctl -n hw.logicalcpu 2>/dev/null || true)"
if [[ ! "$jobs" =~ ^[1-9][0-9]*$ ]]; then
  jobs=4
fi

cd "$build_dir"
PKG_CONFIG_PATH="$x264_isolated/lib/pkgconfig" "$source_dir/configure" \
  --prefix="$install_dir" \
  --arch=arm64 \
  --cc=/usr/bin/clang \
  --pkg-config=pkg-config \
  --pkg-config-flags=--static \
  --enable-gpl \
  --enable-libx264 \
  --enable-videotoolbox \
  --enable-audiotoolbox \
  --enable-static \
  --disable-shared \
  --disable-autodetect \
  --disable-network \
  --disable-doc \
  --disable-debug \
  --disable-ffplay
/usr/bin/make -j "$jobs"
/usr/bin/make install

payload="$temporary_dir/payload"
mkdir -p "$payload"
/usr/bin/install -m 755 "$install_dir/bin/ffmpeg" "$payload/ffmpeg"
/usr/bin/install -m 755 "$install_dir/bin/ffprobe" "$payload/ffprobe"

for tool in "$payload/ffmpeg" "$payload/ffprobe"; do
  if [[ "$(/usr/bin/file -b "$tool")" != *"Mach-O"* ]] || ! /usr/bin/lipo "$tool" -verify_arch arm64 >/dev/null 2>&1; then
    echo "built $(basename "$tool") is not Mach-O ARM64" >&2
    exit 1
  fi
  while IFS= read -r dependency; do
    case "$dependency" in
      /usr/lib/* | /System/Library/*)
        ;;
      *)
        echo "built $(basename "$tool") has non-system dependency: $dependency" >&2
        exit 1
        ;;
    esac
  done < <(/usr/bin/otool -L "$tool" | /usr/bin/awk 'NR > 1 { print $1 }')
  "$tool" -version >/dev/null
done

encoders="$($payload/ffmpeg -hide_banner -encoders 2>/dev/null)"
for encoder in h264_videotoolbox libx264; do
  if ! /usr/bin/grep -q "[[:space:]]${encoder}[[:space:]]" <<<"$encoders"; then
    echo "built ffmpeg is missing required encoder: $encoder" >&2
    exit 1
  fi
done

mkdir -p "$output_dir"
archive="$output_dir/hongguo-media-tools-aarch64-apple-darwin.tar.gz"
archive_temp="$archive.tmp"
/usr/bin/tar -czf "$archive_temp" -C "$payload" ffmpeg ffprobe
/bin/mv "$archive_temp" "$archive"
"$payload/ffmpeg" -L >"$output_dir/ffmpeg-license.txt"

ffmpeg_sha="$(/usr/bin/shasum -a 256 "$payload/ffmpeg" | /usr/bin/awk '{print $1}')"
ffprobe_sha="$(/usr/bin/shasum -a 256 "$payload/ffprobe" | /usr/bin/awk '{print $1}')"
archive_sha="$(/usr/bin/shasum -a 256 "$archive" | /usr/bin/awk '{print $1}')"
metadata="$output_dir/release-metadata.json"
python3 - "$metadata" "$archive" "$archive_sha" "$actual_source_sha" "$ffmpeg_sha" "$ffprobe_sha" <<'PY'
import json
import sys

path, archive, archive_sha, source_sha, ffmpeg_sha, ffprobe_sha = sys.argv[1:]
with open(path, "w", encoding="utf-8") as stream:
    json.dump(
        {
            "archive": archive,
            "archiveSha256": archive_sha,
            "sourceSha256": source_sha,
            "ffmpegSha256": ffmpeg_sha,
            "ffprobeSha256": ffprobe_sha,
        },
        stream,
        indent=2,
    )
    stream.write("\n")
PY

printf '%s\n' "$metadata"
