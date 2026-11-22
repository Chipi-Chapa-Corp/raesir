#!/usr/bin/env bash

set -euo pipefail

readonly branch='stable'
readonly repo_url='https://chipi-chapa-corp.github.io/raesir/flatpak'
readonly bundle_path="${1:-dist/raesir-linux-x64.flatpak}"
readonly pages_root="${2:-dist/flatpak-pages}"
readonly repo_dir="${pages_root}/flatpak"

if [[ ! -f "$bundle_path" ]]; then
  echo "Flatpak bundle not found: $bundle_path" >&2
  exit 1
fi

mkdir -p "$pages_root"
if [[ ! -f "$repo_dir/config" ]]; then
  ostree init --repo="$repo_dir" --mode=archive-z2
fi

flatpak build-import-bundle --update-appstream "$repo_dir" "$bundle_path"
flatpak build-update-repo \
  --title='Raesir' \
  --comment='Stable releases of Raesir' \
  --description='Stable Flatpak releases of the Raesir game launcher' \
  --homepage='https://github.com/Chipi-Chapa-Corp/raesir' \
  --icon="${repo_url}/raesir.png" \
  --default-branch="$branch" \
  --generate-static-deltas \
  "$repo_dir"

cp build/raesir.flatpakrepo build/raesir.flatpakref "$repo_dir/"
cp build/icon.png "$repo_dir/raesir.png"
touch "$pages_root/.nojekyll"
