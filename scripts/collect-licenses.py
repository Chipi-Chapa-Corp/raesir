#!/usr/bin/env python3
"""Include notices for the Rust dependencies and embedded artwork we distribute."""
import json
import pathlib
import shutil
import subprocess
import sys

destination = pathlib.Path(sys.argv[1])
target = subprocess.check_output(["rustc", "-vV"], text=True).split("host: ")[1].splitlines()[0]
metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", target,
], text=True))
resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
for package in metadata["packages"]:
    if package["id"] not in resolved or package["source"] is None:
        continue
    root = pathlib.Path(package["manifest_path"]).parent
    for path in root.iterdir():
        if path.is_file() and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT")):
            output = destination / "rust" / f'{package["name"]}-{package["version"]}' / path.name
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, output)
for path in pathlib.Path("static/icons").rglob("*"):
    if path.is_file() and (path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT")) or "LICENSE" in path.name or path.name == "SOURCES.md"):
        output = destination / "artwork" / path.relative_to("static/icons")
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, output)
