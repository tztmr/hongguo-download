#!/bin/bash
set -euo pipefail

if [[ "$#" != 4 ]]; then
  echo "usage: resolve-release-artifacts.sh <bundle-root> <product> <version> <arch>" >&2
  exit 2
fi

bundle_root="$1"
product="$2"
version="$3"
arch="$4"
for component in "$product" "$version" "$arch"; do
  if [[ -z "$component" || "$component" == */* || "$component" == *$'\n'* ]]; then
    echo "release artifact component is unsafe" >&2
    exit 1
  fi
done

app_path="$bundle_root/macos/$product.app"
dmg_path="$bundle_root/dmg/${product}_${version}_${arch}.dmg"
if [[ ! -d "$app_path" ]]; then
  echo "release build: expected app artifact was not produced" >&2
  exit 1
fi
if [[ ! -f "$dmg_path" || -L "$dmg_path" ]]; then
  echo "release build: expected dmg artifact was not produced" >&2
  exit 1
fi

printf '%s\n%s\n' "$app_path" "$dmg_path"
