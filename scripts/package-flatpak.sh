#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

# Vendor the locked dependency graph before entering the networkless SDK build.
# Use a staging tree so neither target/ nor the repository's Git data is copied.
mkdir -p dist
stage=dist/flatpak-source
build_dir=dist/flatpak-build
state_dir="${EXPOSED_GAMES_FLATPAK_STATE_DIR:-dist/flatpak-state}"
repo_dir=dist/flatpak-repo
cache_options=()
scratch_dirs=("$stage" "$build_dir" "$repo_dir")
if [[ -z "${EXPOSED_GAMES_FLATPAK_STATE_DIR:-}" ]]; then
  scratch_dirs+=("$state_dir")
  cache_options+=(--disable-cache)
fi
cleanup() { rm -rf -- "${scratch_dirs[@]}"; }
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
manifest=build/com.chipichapa.exposedgames.json
if [[ -n "${EXPOSED_GAMES_FLATPAK_STATE_DIR:-}" ]]; then
  mkdir -p "$state_dir/cargo-target"
  # Flatpak Builder uses the manifest path to identify its module cache.
  cached_manifest="dist/${manifest##*/}"
  scratch_dirs+=("$cached_manifest")
  # Only the launcher build needs access to the compiled Rust dependencies.
  python3 - "$manifest" "$cached_manifest" "$(realpath -- "$state_dir/cargo-target")" <<'PY'
import json, pathlib, sys
source, destination = map(pathlib.Path, sys.argv[1:3])
manifest = json.loads(source.read_text())
launcher = next(module for module in manifest["modules"] if module["name"] == "exposed-games")
options = launcher.setdefault("build-options", {})
target = "/run/exposed-games-cargo"
options.setdefault("env", {})["CARGO_TARGET_DIR"] = target
options.setdefault("build-args", []).append(f"--bind-mount={target}={sys.argv[3]}")
destination.write_text(json.dumps(manifest))
PY
  manifest="$cached_manifest"
fi
flatpak-builder --force-clean --disable-rofiles-fuse "${cache_options[@]}" \
  --delete-build-dirs --state-dir="$state_dir" --repo="$repo_dir" \
  "$build_dir" "$manifest"
# Verify against the user's Platform, which has fewer libraries than the SDK.
flatpak build --runtime "$build_dir" sh -ec '
  /app/bin/exposed-games --version
  linkage="$(ldd /app/bin/exposed-games /app/lib/libmpv.so.2)"
  if printf "%s\n" "$linkage" | grep "not found"; then exit 1; fi
  test -f /usr/lib/x86_64-linux-gnu/libarchive.so.13
'
flatpak build-bundle --arch=x86_64 \
  --repo-url=https://chipi-chapa-corp.github.io/raesir/flatpak \
  --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$repo_dir" dist/exposed-games-linux-x64.flatpak com.chipichapa.exposedgames stable
cp build/exposed-games.flatpakrepo dist/
