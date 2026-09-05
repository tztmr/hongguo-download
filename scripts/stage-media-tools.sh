#!/bin/bash
set -euo pipefail

: "${HONGGUO_FFMPEG_ARCHIVE:?required}"
: "${HONGGUO_FFMPEG_SHA256:?required}"
: "${HONGGUO_FFPROBE_SHA256:?required}"

project_root="$(cd "$(dirname "$0")/.." && pwd)"
destination_dir="$project_root/desktop/src-tauri/binaries"
temporary_dir="$(mktemp -d "${TMPDIR:-/tmp}/hongguo-media-tools.XXXXXX")"
staged_ffmpeg=""
staged_ffprobe=""
lock_dir=""
lock_owned=0

cleanup() {
  local status=$?
  trap - EXIT
  if [[ -n "$staged_ffmpeg" && ( -e "$staged_ffmpeg" || -L "$staged_ffmpeg" ) ]]; then
    /bin/rm -f "$staged_ffmpeg"
  fi
  if [[ -n "$staged_ffprobe" && ( -e "$staged_ffprobe" || -L "$staged_ffprobe" ) ]]; then
    /bin/rm -f "$staged_ffprobe"
  fi
  if [[ "$lock_owned" == 1 ]]; then
    /bin/rmdir "$lock_dir" 2>/dev/null || true
  fi
  /bin/rm -rf "$temporary_dir"
  exit "$status"
}
trap cleanup EXIT

archive="$HONGGUO_FFMPEG_ARCHIVE"
if [[ ! -f "$archive" ]]; then
  echo "media tool archive does not exist" >&2
  exit 1
fi

extract_dir="$temporary_dir/extracted"
mkdir -p "$extract_dir"
canonical_extract_dir="$(/bin/realpath "$extract_dir")"

reject_link_members() {
  local member listing count kind
  for member in ffmpeg ffprobe; do
    case "$archive" in
      *.zip)
        count="$(/usr/bin/zipinfo -1 "$archive" "$member" | /usr/bin/awk 'END { print NR }')"
        listing="$(/usr/bin/zipinfo -s "$archive" "$member")"
        ;;
      *)
        count="$(/usr/bin/tar -tf "$archive" "$member" | /usr/bin/awk 'END { print NR }')"
        listing="$(/usr/bin/tar -tvf "$archive" "$member")"
        ;;
    esac
    kind="${listing:0:1}"
    if [[ "$count" != 1 || "$kind" != "-" ]]; then
      echo "archive member must be one regular file: $member" >&2
      exit 1
    fi
  done
}

reject_link_members
case "$archive" in
  *.zip)
    /usr/bin/unzip -qq "$archive" ffmpeg ffprobe -d "$extract_dir"
    ;;
  *)
    /usr/bin/tar -xf "$archive" -C "$extract_dir" ffmpeg ffprobe
    ;;
esac

verify_single_link_regular() {
  local path="$1"
  local links
  if [[ -L "$path" || ! -f "$path" ]]; then
    echo "$(basename "$path") must be a regular non-link file" >&2
    exit 1
  fi
  links="$(/usr/bin/stat -f '%l' "$path")"
  if [[ "$links" != 1 ]]; then
    echo "$(basename "$path") must not be hard-linked" >&2
    exit 1
  fi
}

verify_sha256() {
  local path="$1"
  local expected="$2"
  local actual
  actual="$(/usr/bin/shasum -a 256 "$path" | /usr/bin/awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "SHA-256 mismatch for $(basename "$path")" >&2
    exit 1
  fi
}

verify_arm64_macho() {
  local path="$1"
  local description
  description="$(/usr/bin/file -b "$path")"
  if [[ "$description" != *"Mach-O"* ]] || ! /usr/bin/lipo "$path" -verify_arch arm64 >/dev/null 2>&1; then
    echo "$(basename "$path") is not a Mach-O ARM64 executable" >&2
    exit 1
  fi
}

is_safe_relative_suffix() {
  local suffix="$1"
  [[ -n "$suffix" && "$suffix" != /* && "$suffix" != *//* ]] || return 1
  case "/$suffix/" in
    */./* | */../*)
      return 1
      ;;
  esac
}

canonicalize_absolute_path() {
  local path="$1"
  local probe resolved suffix="" component
  [[ "$path" == /* && "$path" != *//* ]] || return 1
  case "$path/" in
    */./* | */../*)
      return 1
      ;;
  esac

  probe="${path%/}"
  [[ -n "$probe" ]] || probe="/"
  while [[ ! -e "$probe" && ! -L "$probe" ]]; do
    component="${probe##*/}"
    [[ -n "$component" ]] || return 1
    if [[ -n "$suffix" ]]; then
      suffix="$component/$suffix"
    else
      suffix="$component"
    fi
    probe="${probe%/*}"
    [[ -n "$probe" ]] || probe="/"
  done
  resolved="$(/bin/realpath "$probe" 2>/dev/null)" || return 1
  if [[ -n "$suffix" ]]; then
    resolved="${resolved%/}/$suffix"
  fi
  printf '%s\n' "$resolved"
}

is_within_root() {
  local path="$1"
  local root="$2"
  case "$path" in
    "$root" | "$root"/*)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

is_system_path() {
  local path="$1"
  is_within_root "$path" "/usr/lib" || is_within_root "$path" "/System/Library"
}

resolve_loader_path() {
  local reference="$1"
  local tool_dir="$2"
  local suffix candidate resolved
  case "$reference" in
    @loader_path/*)
      suffix="${reference#@loader_path/}"
      ;;
    @executable_path/*)
      suffix="${reference#@executable_path/}"
      ;;
    *)
      return 1
      ;;
  esac
  is_safe_relative_suffix "$suffix" || return 1
  candidate="$tool_dir/$suffix"
  resolved="$(/bin/realpath "$candidate" 2>/dev/null)" || return 1
  is_within_root "$resolved" "$canonical_extract_dir" || return 1
  printf '%s\n' "$resolved"
}

verify_macho_dependencies() {
  local tool="$1"
  local tool_name tool_dir dependency rpath resolved found
  local dependencies_file rpaths_file
  tool_name="$(basename "$tool")"
  tool_dir="$(dirname "$tool")"
  dependencies_file="$temporary_dir/$tool_name.dependencies"
  rpaths_file="$temporary_dir/$tool_name.rpaths"

  /usr/bin/otool -L "$tool" | /usr/bin/awk 'NR > 1 { print $1 }' >"$dependencies_file"
  /usr/bin/otool -l "$tool" | /usr/bin/awk '
    $1 == "cmd" && $2 == "LC_RPATH" { in_rpath = 1; next }
    in_rpath && $1 == "path" { print $2; in_rpath = 0 }
  ' >"$rpaths_file"

  while IFS= read -r rpath; do
    [[ -z "$rpath" ]] && continue
    case "$rpath" in
      @loader_path | @executable_path)
        ;;
      @loader_path/* | @executable_path/*)
        resolved="$(resolve_loader_path "$rpath" "$tool_dir")" || {
          echo "$tool_name has unsafe LC_RPATH: $rpath" >&2
          exit 1
        }
        [[ -d "$resolved" ]] || {
          echo "$tool_name has non-directory LC_RPATH: $rpath" >&2
          exit 1
        }
        ;;
      /*)
        resolved="$(canonicalize_absolute_path "$rpath")" || {
          echo "$tool_name has unsafe LC_RPATH: $rpath" >&2
          exit 1
        }
        is_system_path "$resolved" || {
          echo "$tool_name has unsafe LC_RPATH: $rpath" >&2
          exit 1
        }
        ;;
      *)
        echo "$tool_name has unsafe LC_RPATH: $rpath" >&2
        exit 1
        ;;
    esac
  done <"$rpaths_file"

  while IFS= read -r dependency; do
    [[ -z "$dependency" ]] && continue
    case "$dependency" in
      @loader_path/* | @executable_path/*)
        resolved="$(resolve_loader_path "$dependency" "$tool_dir")" || {
          echo "$tool_name has unsafe loader-relative dependency: $dependency" >&2
          exit 1
        }
        verify_single_link_regular "$resolved"
        ;;
      @rpath/*)
        is_safe_relative_suffix "${dependency#@rpath/}" || {
          echo "$tool_name has unsafe @rpath dependency: $dependency" >&2
          exit 1
        }
        found=0
        while IFS= read -r rpath; do
          case "$rpath" in
            @loader_path | @executable_path)
              rpath="${rpath}/"
              ;;
          esac
          case "$rpath" in
            @loader_path/* | @executable_path/*)
              resolved="$(resolve_loader_path "${rpath%/}/${dependency#@rpath/}" "$tool_dir")" || continue
              if [[ -f "$resolved" && ! -L "$resolved" ]]; then
                verify_single_link_regular "$resolved"
                found=1
                break
              fi
              ;;
            /*)
              resolved="$(canonicalize_absolute_path "${rpath%/}/${dependency#@rpath/}")" || continue
              if is_system_path "$resolved"; then
                found=1
                break
              fi
              ;;
          esac
        done <"$rpaths_file"
        if [[ "$found" != 1 ]]; then
          echo "$tool_name has unresolved or unsafe @rpath dependency: $dependency" >&2
          exit 1
        fi
        ;;
      /*)
        resolved="$(canonicalize_absolute_path "$dependency")" || {
          echo "$tool_name has unsafe dynamic dependency: $dependency" >&2
          exit 1
        }
        is_system_path "$resolved" || {
          echo "$tool_name has unsafe dynamic dependency: $dependency" >&2
          exit 1
        }
        ;;
      *)
        echo "$tool_name has unsafe dynamic dependency: $dependency" >&2
        exit 1
        ;;
    esac
  done <"$dependencies_file"
}

ffmpeg="$extract_dir/ffmpeg"
ffprobe="$extract_dir/ffprobe"
verify_single_link_regular "$ffmpeg"
verify_single_link_regular "$ffprobe"
verify_sha256 "$ffmpeg" "$HONGGUO_FFMPEG_SHA256"
verify_sha256 "$ffprobe" "$HONGGUO_FFPROBE_SHA256"

for tool in "$ffmpeg" "$ffprobe"; do
  if [[ ! -x "$tool" ]]; then
    echo "$(basename "$tool") is not executable" >&2
    exit 1
  fi
  verify_arm64_macho "$tool"
  verify_macho_dependencies "$tool"
  "$tool" -version >/dev/null
done

encoders="$($ffmpeg -hide_banner -encoders 2>/dev/null)"
for encoder in h264_videotoolbox libx264; do
  if ! /usr/bin/grep -q "[[:space:]]${encoder}[[:space:]]" <<<"$encoders"; then
    echo "ffmpeg is missing required encoder: $encoder" >&2
    exit 1
  fi
done

mkdir -p "$destination_dir"
lock_dir="$destination_dir/.media-tools-stage.lock"
if ! /bin/mkdir "$lock_dir" 2>/dev/null; then
  echo "another media tool staging operation is active" >&2
  exit 1
fi
lock_owned=1

commit_id="$$.$RANDOM"
staged_ffmpeg="$destination_dir/.ffmpeg-aarch64-apple-darwin.$commit_id"
staged_ffprobe="$destination_dir/.ffprobe-aarch64-apple-darwin.$commit_id"
/usr/bin/install -m 755 "$ffmpeg" "$staged_ffmpeg"
/usr/bin/install -m 755 "$ffprobe" "$staged_ffprobe"
verify_single_link_regular "$staged_ffmpeg"
verify_single_link_regular "$staged_ffprobe"
verify_sha256 "$staged_ffmpeg" "$HONGGUO_FFMPEG_SHA256"
verify_sha256 "$staged_ffprobe" "$HONGGUO_FFPROBE_SHA256"
/bin/sync

pair_commit() {
  local final_ffmpeg="$destination_dir/ffmpeg-aarch64-apple-darwin"
  local final_ffprobe="$destination_dir/ffprobe-aarch64-apple-darwin"
  local backup_ffmpeg="$destination_dir/.ffmpeg-aarch64-apple-darwin.backup.$commit_id"
  local backup_ffprobe="$destination_dir/.ffprobe-aarch64-apple-darwin.backup.$commit_id"
  local had_old_pair=0

  if [[ -e "$final_ffmpeg" || -L "$final_ffmpeg" || -e "$final_ffprobe" || -L "$final_ffprobe" ]]; then
    if [[ ! -f "$final_ffmpeg" || -L "$final_ffmpeg" || ! -f "$final_ffprobe" || -L "$final_ffprobe" ]]; then
      echo "existing media tool pair is incomplete or unsafe" >&2
      return 1
    fi
    had_old_pair=1
    if ! /bin/mv "$final_ffmpeg" "$backup_ffmpeg"; then
      return 1
    fi
    if ! /bin/mv "$final_ffprobe" "$backup_ffprobe"; then
      /bin/mv "$backup_ffmpeg" "$final_ffmpeg" || true
      return 1
    fi
  fi

  if ! /bin/mv "$staged_ffmpeg" "$final_ffmpeg"; then
    if [[ "$had_old_pair" == 1 ]]; then
      /bin/mv "$backup_ffmpeg" "$final_ffmpeg" || true
      /bin/mv "$backup_ffprobe" "$final_ffprobe" || true
    fi
    return 1
  fi

  if [[ "${HONGGUO_STAGE_FAIL_AFTER_FFMPEG_COMMIT:-}" == 1 ]] || ! /bin/mv "$staged_ffprobe" "$final_ffprobe"; then
    /bin/rm -f "$final_ffmpeg"
    if [[ "$had_old_pair" == 1 ]]; then
      /bin/mv "$backup_ffmpeg" "$final_ffmpeg" || true
      /bin/mv "$backup_ffprobe" "$final_ffprobe" || true
    fi
    return 1
  fi

  if [[ "$had_old_pair" == 1 ]]; then
    /bin/rm -f "$backup_ffmpeg" "$backup_ffprobe"
  fi
}

if ! pair_commit; then
  echo "media tool pair commit failed; previous pair restored" >&2
  exit 1
fi
