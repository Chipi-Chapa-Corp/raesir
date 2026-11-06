#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

# Run in MSYS2 UCRT64: its Rust compiler and native libraries use the same CRT.
if [[ "${MSYSTEM:-}" != UCRT64 ]]; then
  echo 'Run this script in an MSYS2 UCRT64 shell (see build/README.md).' >&2
  exit 1
fi
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_RELEASE_STRIP=debuginfo
cargo build --release --locked --package raesir --bin raesir
python scripts/package-windows.py
version="$(python -c 'import tomllib; print(tomllib.load(open("native/Cargo.toml", "rb"))["package"]["version"])')"
makensis -WX -DAPP_VERSION="$version" \
  -DPAYLOAD_DIR="$(cygpath -w "$PWD/dist/windows")" \
  -DUNINSTALL_FILES="$(cygpath -w "$PWD/dist/windows-uninstall.nsh")" \
  build/windows.nsi
