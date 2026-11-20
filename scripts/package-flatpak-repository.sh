#!/usr/bin/env bash

set -euo pipefail

readonly branch='stable'
readonly repo_url='https://chipi-chapa-corp.github.io/raesir/flatpak'
readonly bundle_path="${1:-dist/exposed-games-linux-x64.flatpak}"
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
  --title='Exposed Games' \
  --comment='Stable releases of Exposed Games' \
  --description='Stable Flatpak releases of the Exposed Games game launcher' \
  --homepage='https://github.com/Chipi-Chapa-Corp/raesir' \
  --icon="${repo_url}/exposed-games.png" \
  --default-branch="$branch" \
  --generate-static-deltas \
  "$repo_dir"

cp build/exposed-games.flatpakrepo build/exposed-games.flatpakref "$repo_dir/"
cp build/icon.png "$repo_dir/exposed-games.png"
touch "$pages_root/.nojekyll"
