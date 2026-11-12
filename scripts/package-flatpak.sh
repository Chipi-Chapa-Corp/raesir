#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

# Vendor the locked dependency graph before entering the networkless SDK build.
# Use a staging tree so neither target/ nor the repository's Git data is copied.
mkdir -p dist
stage=dist/flatpak-source
build_dir=dist/flatpak-build
state_dir=dist/flatpak-state
repo_dir=dist/flatpak-repo
cleanup() { rm -rf -- "$stage" "$build_dir" "$state_dir" "$repo_dir"; }
trap cleanup EXIT
cleanup
mkdir -p "$stage/.cargo"
cp Cargo.toml Cargo.lock LICENSE "$stage/"
cp -R native backend static build scripts "$stage/"
cargo vendor --locked --versioned-dirs "$stage/vendor" > "$stage/.cargo/config.toml"
# cargo vendor prints a host path; the SDK needs a path relative to its sources.
python3 - "$stage/.cargo/config.toml" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
lines = path.read_text().splitlines()
path.write_text('\n'.join('directory = "vendor"' if line.startswith('directory = ') else line for line in lines) + '\n')
PY
flatpak-builder --force-clean --disable-rofiles-fuse --disable-cache \
  --delete-build-dirs --state-dir="$state_dir" --repo="$repo_dir" \
  "$build_dir" build/com.chipichapa.exposedgames.json
# Verify against the user's Platform, which has fewer libraries than the SDK.
flatpak build --runtime "$build_dir" sh -ec '
  /app/bin/exposed-games --version
  linkage="$(ldd /app/bin/exposed-games /app/lib/libmpv.so.2)"
  if printf "%s\n" "$linkage" | grep "not found"; then exit 1; fi
  test -f /usr/lib/x86_64-linux-gnu/libarchive.so.13
'
flatpak build-bundle --arch=x86_64 \
  --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$repo_dir" dist/exposed-games-linux-x64.flatpak com.chipichapa.exposedgames stable
